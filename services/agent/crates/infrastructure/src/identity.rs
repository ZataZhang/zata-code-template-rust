//! 调用身份与 Runtime 凭据解析。
//!
//! - 调用服务：部署注入的 Bearer token 清单，服务端只保存 SHA-256 摘要；
//! - public 用户与管理员：分别经 accounts `GET /auth/me` 与 `GET /admin/auth/me`
//!   校验 Cookie，两种身份不合并；
//! - Runtime 凭据别名：真实密钥只从部署环境按别名读取，不写入数据库或快照。

use std::collections::BTreeMap;
use std::env;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::StatusCode;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tpl_agent_core::error::AgentError;
use tpl_agent_core::identity::{
    AdminIdentity, AdminIdentityValidator, ServicePrincipal, ServiceTokenValidator, UserIdentity,
    UserIdentityValidator,
};
use tpl_agent_core::ports::CredentialResolver;

const MIN_TOKEN_BYTES: usize = 32;
const MAX_TOKEN_BYTES: usize = 4096;
const MAX_COOKIE_HEADER_BYTES: usize = 8 * 1024;
const MAX_IDENTITY_RESPONSE_BYTES: usize = 16 * 1024;

/// 逐服务 Bearer 凭据登记表；只保存 `SHA-256(token)`，比较不接触明文。
#[derive(Clone, Default)]
pub struct ServiceTokenRegistry {
    token_hashes: BTreeMap<String, [u8; 32]>,
}

impl ServiceTokenRegistry {
    /// 由 `service_id -> token` 清单构造；token 长度与字符已由配置层校验。
    pub fn new(service_tokens: BTreeMap<String, String>) -> Self {
        let token_hashes = service_tokens
            .into_iter()
            .map(|(service_id, token)| {
                let token_hash: [u8; 32] = Sha256::digest(token.as_bytes()).into();
                (service_id, token_hash)
            })
            .collect();
        Self { token_hashes }
    }
}

impl ServiceTokenValidator for ServiceTokenRegistry {
    fn authenticate_service_token(
        &self,
        authorization_header: Option<&str>,
    ) -> Result<ServicePrincipal, AgentError> {
        let bearer_token = authorization_header
            .and_then(|header_value| header_value.strip_prefix("Bearer "))
            .filter(|token| {
                (MIN_TOKEN_BYTES..=MAX_TOKEN_BYTES).contains(&token.len())
                    && !token.chars().any(char::is_control)
            })
            .ok_or(AgentError::ServiceUnauthorized)?;
        let presented_hash: [u8; 32] = Sha256::digest(bearer_token.as_bytes()).into();
        self.token_hashes
            .iter()
            .find(|(_, registered_hash)| **registered_hash == presented_hash)
            .map(|(service_id, _)| ServicePrincipal {
                service_id: service_id.clone(),
            })
            .ok_or(AgentError::ServiceUnauthorized)
    }
}

/// accounts `/auth/me` 响应的最小契约。
#[derive(Deserialize)]
struct UserMeResponse {
    user: UserMeBody,
}

#[derive(Deserialize)]
struct UserMeBody {
    id: i64,
    email: String,
}

/// accounts `/admin/auth/me` 响应的最小契约；管理员 ID 以字符串返回。
#[derive(Deserialize)]
struct AdminMeResponse {
    user_id: String,
    display_name: String,
}

/// 通过 accounts 公开 HTTP 接口校验 Cookie 的身份客户端。
#[derive(Clone)]
pub struct AccountsIdentityClient {
    client: reqwest::Client,
    user_auth_url: String,
    admin_auth_url: String,
}

impl AccountsIdentityClient {
    /// 创建带超时与禁用重定向的客户端。
    ///
    /// # Errors
    ///
    /// HTTP 客户端无法建立时返回错误。
    pub fn new(
        user_auth_url: String,
        admin_auth_url: String,
        request_timeout: Duration,
    ) -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .timeout(request_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            client,
            user_auth_url,
            admin_auth_url,
        })
    }

    async fn request_identity(
        &self,
        auth_url: &str,
        cookie_header: Option<&str>,
    ) -> Result<Option<Vec<u8>>, AgentError> {
        let cookie_header = cookie_header
            .filter(|header_value| {
                !header_value.is_empty()
                    && header_value.len() <= MAX_COOKIE_HEADER_BYTES
                    && !header_value.chars().any(char::is_control)
            })
            .ok_or(AgentError::Unauthenticated)?;
        let mut identity_response = self
            .client
            .get(auth_url)
            .header(reqwest::header::COOKIE, cookie_header)
            .send()
            .await
            .map_err(|error| {
                tracing::warn!(error = %error, "accounts 身份校验请求失败");
                AgentError::IdentityUnavailable
            })?;
        if matches!(
            identity_response.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) {
            return Ok(None);
        }
        if !identity_response.status().is_success()
            || identity_response
                .content_length()
                .is_some_and(|body_size| body_size > MAX_IDENTITY_RESPONSE_BYTES as u64)
        {
            return Err(AgentError::IdentityUnavailable);
        }
        let mut response_body = Vec::new();
        while let Some(response_chunk) = identity_response.chunk().await.map_err(|error| {
            tracing::warn!(error = %error, "读取 accounts 身份校验响应失败");
            AgentError::IdentityUnavailable
        })? {
            if response_body.len().saturating_add(response_chunk.len())
                > MAX_IDENTITY_RESPONSE_BYTES
            {
                return Err(AgentError::IdentityUnavailable);
            }
            response_body.extend_from_slice(&response_chunk);
        }
        Ok(Some(response_body))
    }
}

#[async_trait]
impl UserIdentityValidator for AccountsIdentityClient {
    async fn authenticate_user(
        &self,
        cookie_header: Option<&str>,
    ) -> Result<UserIdentity, AgentError> {
        let response_body = self
            .request_identity(&self.user_auth_url, cookie_header)
            .await?
            .ok_or(AgentError::Unauthenticated)?;
        let user_response = serde_json::from_slice::<UserMeResponse>(&response_body)
            .map_err(|_| AgentError::IdentityUnavailable)?;
        if user_response.user.id <= 0 || user_response.user.email.chars().any(char::is_control) {
            return Err(AgentError::IdentityUnavailable);
        }
        Ok(UserIdentity {
            user_id: user_response.user.id,
            email: user_response.user.email,
        })
    }
}

#[async_trait]
impl AdminIdentityValidator for AccountsIdentityClient {
    async fn authenticate_admin(
        &self,
        cookie_header: Option<&str>,
    ) -> Result<AdminIdentity, AgentError> {
        let response_body = self
            .request_identity(&self.admin_auth_url, cookie_header)
            .await?
            .ok_or(AgentError::Unauthenticated)?;
        let admin_response = serde_json::from_slice::<AdminMeResponse>(&response_body)
            .map_err(|_| AgentError::IdentityUnavailable)?;
        let admin_user_id = admin_response
            .user_id
            .trim()
            .parse::<i64>()
            .ok()
            .filter(|admin_user_id| *admin_user_id > 0)
            .ok_or(AgentError::IdentityUnavailable)?;
        if admin_response.display_name.chars().any(char::is_control) {
            return Err(AgentError::IdentityUnavailable);
        }
        Ok(AdminIdentity {
            admin_user_id,
            display_name: admin_response.display_name,
        })
    }
}

/// 从环境变量解析 Runtime 凭据别名。
///
/// 别名 `corp-a2a` 对应 `AGENT_CREDENTIAL_CORP_A2A`；别名字符只允许
/// 字母、数字、`-` 和 `_`，保证映射无歧义且不会读取任意变量。
#[derive(Clone, Copy, Debug, Default)]
pub struct EnvironmentCredentialResolver;

impl CredentialResolver for EnvironmentCredentialResolver {
    fn resolve(&self, alias: &str) -> Option<String> {
        resolve_credential_with(alias, |variable_name| env::var(variable_name).ok())
    }
}

/// 将凭据别名映射为环境变量名。
fn credential_variable_name(alias: &str) -> Option<String> {
    let alias_is_valid = !alias.is_empty()
        && alias.len() <= 128
        && alias
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'));
    if !alias_is_valid {
        return None;
    }
    let variable_suffix = alias
        .chars()
        .map(|character| match character {
            'a'..='z' => character.to_ascii_uppercase(),
            'A'..='Z' | '0'..='9' => character,
            '-' | '_' => '_',
            _ => '_',
        })
        .collect::<String>();
    Some(format!("AGENT_CREDENTIAL_{variable_suffix}"))
}

fn resolve_credential_with(
    alias: &str,
    lookup: impl FnOnce(&str) -> Option<String>,
) -> Option<String> {
    let variable_name = credential_variable_name(alias)?;
    lookup(&variable_name)
        .map(|secret| secret.trim().to_owned())
        .filter(|secret| !secret.is_empty() && secret.len() <= MAX_TOKEN_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> ServiceTokenRegistry {
        ServiceTokenRegistry::new(BTreeMap::from([(
            "web".to_owned(),
            "0123456789abcdef0123456789abcdef".to_owned(),
        )]))
    }

    #[test]
    fn valid_bearer_token_resolves_service_identity() {
        let principal = registry()
            .authenticate_service_token(Some("Bearer 0123456789abcdef0123456789abcdef"))
            .expect("正确 token 应当通过");
        assert_eq!(principal.service_id, "web");
    }

    #[test]
    fn missing_short_or_unknown_tokens_are_rejected() {
        let registry = registry();
        for header in [
            None,
            Some("Basic abc"),
            Some("Bearer short"),
            Some("Bearer ffffffffffffffffffffffffffffffff"),
        ] {
            assert_eq!(
                registry.authenticate_service_token(header),
                Err(AgentError::ServiceUnauthorized)
            );
        }
    }

    #[test]
    fn credential_alias_maps_to_environment_variable() {
        assert_eq!(
            credential_variable_name("corp-a2a").as_deref(),
            Some("AGENT_CREDENTIAL_CORP_A2A")
        );
        let resolved = resolve_credential_with("corp-a2a", |variable_name| {
            assert_eq!(variable_name, "AGENT_CREDENTIAL_CORP_A2A");
            Some("  secret-value  ".to_owned())
        });
        assert_eq!(resolved.as_deref(), Some("secret-value"));
        assert!(credential_variable_name("../escape").is_none());
        assert!(resolve_credential_with("corp/a2a", |_| Some("x".to_owned())).is_none());
    }
}
