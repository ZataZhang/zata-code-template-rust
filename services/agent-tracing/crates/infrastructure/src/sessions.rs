//! 通过 accounts-service 的公开接口校验管理员会话。

use std::time::Duration;

use async_trait::async_trait;
use reqwest::StatusCode;
use serde::Deserialize;
use tpl_agent_tracing_core::ports::{AdminAuthError, AdminIdentity, AdminSessionValidator};

const MAX_COOKIE_HEADER_BYTES: usize = 4 * 1024;
const MAX_ADMIN_COOKIE_BYTES: usize = 256;
const MAX_ADMIN_RESPONSE_BYTES: usize = 8 * 1024;

/// accounts `/admin/auth/me` 响应的最小契约。
#[derive(Deserialize)]
struct AdminResponse {
    /// 已认证管理员标识。
    user_id: String,
}

/// 使用 accounts-service HTTP 接口验证管理员 Cookie。
pub struct AccountsAdminSessionValidator {
    client: reqwest::Client,
    admin_auth_url: String,
    admin_cookie_name: String,
}

impl AccountsAdminSessionValidator {
    /// 创建带超时、无重定向和无凭据日志的 accounts 客户端。
    ///
    /// # Errors
    ///
    /// HTTP 客户端配置无法建立时返回错误。
    pub fn new(
        admin_auth_url: String,
        admin_cookie_name: String,
        request_timeout: Duration,
    ) -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .timeout(request_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            client,
            admin_auth_url,
            admin_cookie_name,
        })
    }
}

#[async_trait]
impl AdminSessionValidator for AccountsAdminSessionValidator {
    async fn authenticate(
        &self,
        cookie_header: Option<&str>,
    ) -> Result<AdminIdentity, AdminAuthError> {
        let cookie_value = cookie_header
            .filter(|header_value| header_value.len() <= MAX_COOKIE_HEADER_BYTES)
            .and_then(|header_value| {
                header_value.split(';').find_map(|cookie_pair| {
                    let (cookie_name, cookie_value) = cookie_pair.trim().split_once('=')?;
                    (cookie_name == self.admin_cookie_name).then_some(cookie_value.trim())
                })
            })
            .filter(|cookie_value| {
                !cookie_value.is_empty()
                    && cookie_value.len() <= MAX_ADMIN_COOKIE_BYTES
                    && !cookie_value.chars().any(char::is_control)
            })
            .ok_or(AdminAuthError::Unauthenticated)?;
        let outbound_cookie = format!("{}={cookie_value}", self.admin_cookie_name);
        let mut accounts_response = self
            .client
            .get(&self.admin_auth_url)
            .header(reqwest::header::COOKIE, outbound_cookie)
            .send()
            .await
            .map_err(|_| AdminAuthError::Unavailable)?;
        if matches!(
            accounts_response.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) {
            return Err(AdminAuthError::Unauthenticated);
        }
        if !accounts_response.status().is_success()
            || accounts_response
                .content_length()
                .is_some_and(|body_size| body_size > MAX_ADMIN_RESPONSE_BYTES as u64)
        {
            return Err(AdminAuthError::Unavailable);
        }
        let mut response_body = Vec::new();
        while let Some(response_chunk) = accounts_response
            .chunk()
            .await
            .map_err(|_| AdminAuthError::Unavailable)?
        {
            if response_body.len().saturating_add(response_chunk.len()) > MAX_ADMIN_RESPONSE_BYTES {
                return Err(AdminAuthError::Unavailable);
            }
            response_body.extend_from_slice(&response_chunk);
        }
        let admin_response = serde_json::from_slice::<AdminResponse>(&response_body)
            .map_err(|_| AdminAuthError::Unavailable)?;
        if admin_response.user_id.trim().is_empty()
            || admin_response.user_id.len() > 128
            || admin_response.user_id.chars().any(char::is_control)
        {
            return Err(AdminAuthError::Unavailable);
        }
        Ok(AdminIdentity {
            user_id: admin_response.user_id,
        })
    }
}
