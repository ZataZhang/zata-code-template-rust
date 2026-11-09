//! 消费服务认证：Bearer 服务凭据 + 短时 HMAC-SHA-256 用户上下文。
//!
//! 用户上下文格式为 `<base64url(payload_json)>.<base64url(hmac_sha256)>`，
//! 签名覆盖第一段的 ASCII 文本。服务 token 与签名密钥按消费服务独立配置，
//! token 只比较 SHA-256 摘要，避免日志或内存出现明文。

use std::collections::BTreeMap;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use time::OffsetDateTime;
use tpl_mcp_core::{
    AuthError, CallerAuthenticator, CallerRole, ConsumerId, GroupId, IdentityContext, SubjectId,
    TenantId,
};

use crate::config::SecuritySettings;

/// 服务 token 最大字节数。
const MAX_TOKEN_BYTES: usize = 4096;
/// 用户上下文最大字节数。
const MAX_CONTEXT_BYTES: usize = 8192;
/// 单个用户允许携带的最大分组数。
const MAX_GROUPS: usize = 100;

/// 单个消费服务的认证材料。
#[derive(Clone)]
struct ConsumerSecret {
    token_sha256: [u8; 32],
    signing_secret: Vec<u8>,
}

/// 消费服务配置无效；错误信息不回显任何密钥。
#[derive(Debug, thiserror::Error)]
#[error("消费服务配置无效")]
pub struct AuthConfigurationError;

/// 验证消费服务 Bearer token 与短时签名用户上下文。
pub struct HmacCallerAuthenticator {
    consumers: BTreeMap<String, ConsumerSecret>,
    issuer: String,
    audience: String,
    max_context_age_secs: i64,
    clock_skew_secs: i64,
}

impl HmacCallerAuthenticator {
    /// 从服务配置构造认证器。
    ///
    /// # Errors
    ///
    /// 消费方 JSON 结构错误、ID 重复、token 或签名密钥过短时返回错误。
    pub fn from_settings(settings: &SecuritySettings) -> Result<Self, AuthConfigurationError> {
        #[derive(Deserialize)]
        struct ConsumerEntry {
            consumer_id: String,
            token: String,
            context_signing_secret: String,
        }

        if settings.context_max_age_secs <= 0 || settings.context_max_age_secs > 3600 {
            return Err(AuthConfigurationError);
        }
        if !(0..=300).contains(&settings.context_clock_skew_secs) {
            return Err(AuthConfigurationError);
        }
        if settings.context_issuer.trim().is_empty() || settings.context_audience.trim().is_empty()
        {
            return Err(AuthConfigurationError);
        }

        let entries = serde_json::from_str::<Vec<ConsumerEntry>>(&settings.consumers_json)
            .map_err(|_| AuthConfigurationError)?;
        if entries.is_empty() {
            return Err(AuthConfigurationError);
        }
        let mut consumers = BTreeMap::new();
        for entry in entries {
            if ConsumerId::new(entry.consumer_id.clone()).is_err()
                || entry.token.len() < 32
                || entry.token.len() > MAX_TOKEN_BYTES
                || entry.token.chars().any(char::is_control)
                || entry.context_signing_secret.len() < 32
                || entry.context_signing_secret.len() > MAX_TOKEN_BYTES
                || entry.context_signing_secret.chars().any(char::is_control)
            {
                return Err(AuthConfigurationError);
            }
            let token_sha256 = Sha256::digest(entry.token.as_bytes()).into();
            let replaced = consumers.insert(
                entry.consumer_id,
                ConsumerSecret {
                    token_sha256,
                    signing_secret: entry.context_signing_secret.into_bytes(),
                },
            );
            if replaced.is_some() {
                return Err(AuthConfigurationError);
            }
        }

        Ok(Self {
            consumers,
            issuer: settings.context_issuer.clone(),
            audience: settings.context_audience.clone(),
            max_context_age_secs: settings.context_max_age_secs,
            clock_skew_secs: settings.context_clock_skew_secs,
        })
    }

    fn verify_user_context(
        &self,
        consumer_id: &str,
        consumer: &ConsumerSecret,
        user_context: &str,
    ) -> Result<IdentityContext, AuthError> {
        if user_context.len() > MAX_CONTEXT_BYTES {
            return Err(AuthError::InvalidUserContext);
        }
        let (payload_segment, signature_segment) = user_context
            .split_once('.')
            .ok_or(AuthError::InvalidUserContext)?;
        if signature_segment.contains('.') {
            return Err(AuthError::InvalidUserContext);
        }
        let payload = URL_SAFE_NO_PAD
            .decode(payload_segment)
            .map_err(|_| AuthError::InvalidUserContext)?;
        let signature = URL_SAFE_NO_PAD
            .decode(signature_segment)
            .map_err(|_| AuthError::InvalidUserContext)?;
        if payload.len() > MAX_CONTEXT_BYTES {
            return Err(AuthError::InvalidUserContext);
        }
        let mut verifier = Hmac::<Sha256>::new_from_slice(&consumer.signing_secret)
            .map_err(|_| AuthError::InvalidUserContext)?;
        verifier.update(payload_segment.as_bytes());
        verifier
            .verify_slice(&signature)
            .map_err(|_| AuthError::InvalidUserContext)?;

        let claims = serde_json::from_slice::<UserContextClaims>(&payload)
            .map_err(|_| AuthError::InvalidUserContext)?;
        self.validate_claims(claims, consumer_id)
    }

    fn validate_claims(
        &self,
        claims: UserContextClaims,
        consumer_id: &str,
    ) -> Result<IdentityContext, AuthError> {
        if claims.iss != self.issuer || claims.aud != self.audience {
            return Err(AuthError::InvalidUserContext);
        }
        if claims.consumer_id != consumer_id {
            return Err(AuthError::ScopeMismatch);
        }
        if claims.exp <= claims.iat || claims.exp - claims.iat > self.max_context_age_secs {
            return Err(AuthError::InvalidUserContext);
        }
        let now = OffsetDateTime::now_utc().unix_timestamp();
        if claims.iat > now.saturating_add(self.clock_skew_secs)
            || claims.exp < now.saturating_sub(self.clock_skew_secs)
        {
            return Err(AuthError::ExpiredUserContext);
        }
        if claims.groups.len() > MAX_GROUPS {
            return Err(AuthError::InvalidUserContext);
        }

        let consumer_id =
            ConsumerId::new(claims.consumer_id).map_err(|_| AuthError::InvalidUserContext)?;
        let tenant_id =
            TenantId::new(claims.tenant_id).map_err(|_| AuthError::InvalidUserContext)?;
        let subject_id =
            SubjectId::new(claims.subject_id).map_err(|_| AuthError::InvalidUserContext)?;
        let role = CallerRole::parse(&claims.role).ok_or(AuthError::InvalidUserContext)?;
        let mut groups = claims
            .groups
            .into_iter()
            .map(GroupId::new)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| AuthError::InvalidUserContext)?;
        groups.sort();
        groups.dedup();

        Ok(IdentityContext {
            consumer_id,
            tenant_id,
            subject_id,
            groups,
            role,
        })
    }
}

#[async_trait]
impl CallerAuthenticator for HmacCallerAuthenticator {
    async fn authenticate(
        &self,
        authorization: Option<&str>,
        user_context: Option<&str>,
    ) -> Result<IdentityContext, AuthError> {
        let authorization = authorization.ok_or(AuthError::MissingServiceToken)?;
        let token = parse_bearer_token(authorization)?;
        let supplied_digest = Sha256::digest(token.as_bytes());

        // 遍历全部消费方，避免通过耗时差异判断 token 属于哪个服务。
        let mut matched = None;
        for (consumer_id, consumer) in &self.consumers {
            if bool::from(
                supplied_digest
                    .as_slice()
                    .ct_eq(consumer.token_sha256.as_slice()),
            ) {
                matched = Some((consumer_id.as_str(), consumer));
            }
        }
        let (consumer_id, consumer) = matched.ok_or(AuthError::InvalidServiceToken)?;

        let user_context = user_context
            .filter(|value| !value.trim().is_empty())
            .ok_or(AuthError::MissingUserContext)?;
        self.verify_user_context(consumer_id, consumer, user_context)
    }
}

/// 解析 `Bearer` 方案；方案名大小写不敏感，token 不允许空白或控制字符。
fn parse_bearer_token(authorization: &str) -> Result<&str, AuthError> {
    let (scheme, token) = authorization
        .split_once(' ')
        .ok_or(AuthError::InvalidServiceToken)?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return Err(AuthError::InvalidServiceToken);
    }
    let token = token.trim();
    if token.is_empty() || token.len() > MAX_TOKEN_BYTES || token.chars().any(char::is_control) {
        return Err(AuthError::InvalidServiceToken);
    }
    Ok(token)
}

/// 用户上下文声明。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct UserContextClaims {
    /// 签发方。
    iss: String,
    /// 接收方。
    aud: String,
    /// 消费服务 ID，必须与 Bearer token 映射一致。
    consumer_id: String,
    /// 租户 ID。
    tenant_id: String,
    /// 用户主体 ID。
    subject_id: String,
    /// 用户所属分组。
    #[serde(default)]
    groups: Vec<String>,
    /// 调用方角色。
    role: String,
    /// 签发时间（Unix 秒）。
    iat: i64,
    /// 过期时间（Unix 秒）。
    exp: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LogFormat;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";
    const SIGNING_SECRET: &str = "abcdef0123456789abcdef0123456789";

    fn settings_with_signing_secret(secret: &str) -> SecuritySettings {
        SecuritySettings {
            consumers_json: format!(
                r#"[{{"consumer_id":"consumer-a","token":"{TOKEN}","context_signing_secret":"{secret}"}}]"#
            ),
            ..SecuritySettings::default()
        }
    }

    fn authenticator() -> HmacCallerAuthenticator {
        HmacCallerAuthenticator::from_settings(&settings_with_signing_secret(SIGNING_SECRET))
            .expect("测试配置有效")
    }

    fn claims() -> UserContextClaims {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        UserContextClaims {
            iss: "tpl-consumer-backend".to_owned(),
            aud: "tpl-mcp-gateway".to_owned(),
            consumer_id: "consumer-a".to_owned(),
            tenant_id: "tenant-a".to_owned(),
            subject_id: "user-1".to_owned(),
            groups: vec!["group-1".to_owned(), "group-1".to_owned()],
            role: "user".to_owned(),
            iat: now,
            exp: now + 60,
        }
    }

    fn sign(claims: &UserContextClaims, secret: &str) -> String {
        let payload = serde_json::to_vec(claims).expect("测试声明应当可序列化");
        let payload_segment = URL_SAFE_NO_PAD.encode(payload);
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("测试密钥有效");
        mac.update(payload_segment.as_bytes());
        let signature_segment = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        format!("{payload_segment}.{signature_segment}")
    }

    #[tokio::test]
    async fn valid_context_is_accepted_and_groups_are_deduplicated() {
        let identity = authenticator()
            .authenticate(
                Some(&format!("Bearer {TOKEN}")),
                Some(&sign(&claims(), SIGNING_SECRET)),
            )
            .await
            .expect("有效上下文应当通过");
        assert_eq!(identity.consumer_id.as_str(), "consumer-a");
        assert_eq!(identity.tenant_id.as_str(), "tenant-a");
        assert_eq!(identity.subject_id.as_str(), "user-1");
        assert_eq!(identity.groups.len(), 1);
        assert_eq!(identity.role, CallerRole::User);
    }

    #[tokio::test]
    async fn wrong_secret_wrong_consumer_and_expired_contexts_are_rejected() {
        let authenticator = authenticator();
        let forged = sign(&claims(), "ffffffffffffffffffffffffffffffff");
        assert_eq!(
            authenticator
                .authenticate(Some(&format!("Bearer {TOKEN}")), Some(&forged))
                .await,
            Err(AuthError::InvalidUserContext)
        );

        let mut other_consumer = claims();
        other_consumer.consumer_id = "consumer-b".to_owned();
        assert_eq!(
            authenticator
                .authenticate(
                    Some(&format!("Bearer {TOKEN}")),
                    Some(&sign(&other_consumer, SIGNING_SECRET))
                )
                .await,
            Err(AuthError::ScopeMismatch)
        );

        let mut expired = claims();
        expired.iat -= 120;
        expired.exp -= 120;
        assert_eq!(
            authenticator
                .authenticate(
                    Some(&format!("Bearer {TOKEN}")),
                    Some(&sign(&expired, SIGNING_SECRET))
                )
                .await,
            Err(AuthError::ExpiredUserContext)
        );
    }

    #[tokio::test]
    async fn missing_or_invalid_service_tokens_are_rejected() {
        let authenticator = authenticator();
        let context = sign(&claims(), SIGNING_SECRET);
        assert_eq!(
            authenticator.authenticate(None, Some(&context)).await,
            Err(AuthError::MissingServiceToken)
        );
        assert_eq!(
            authenticator
                .authenticate(
                    Some("Bearer wrong-token-wrong-token-wrong-token"),
                    Some(&context)
                )
                .await,
            Err(AuthError::InvalidServiceToken)
        );
        assert_eq!(
            authenticator
                .authenticate(Some("Basic abc"), Some(&context))
                .await,
            Err(AuthError::InvalidServiceToken)
        );
    }

    #[test]
    fn configuration_rejects_short_secrets_and_duplicates() {
        assert!(
            HmacCallerAuthenticator::from_settings(&settings_with_signing_secret("short")).is_err()
        );
        let empty = SecuritySettings {
            consumers_json: "[]".to_owned(),
            ..SecuritySettings::default()
        };
        assert!(HmacCallerAuthenticator::from_settings(&empty).is_err());
        let duplicate = format!(
            r#"[{{"consumer_id":"consumer-a","token":"{TOKEN}","context_signing_secret":"{SIGNING_SECRET}"}},{{"consumer_id":"consumer-a","token":"{TOKEN}","context_signing_secret":"{SIGNING_SECRET}"}}]"#
        );
        let settings = SecuritySettings {
            consumers_json: duplicate,
            ..SecuritySettings::default()
        };
        assert!(HmacCallerAuthenticator::from_settings(&settings).is_err());
        assert_eq!(LogFormat::default(), LogFormat::Text);
    }
}
