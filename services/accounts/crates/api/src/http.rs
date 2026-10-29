//! 各路由共用的 HTTP 细节：错误响应、客户端 IP、会话 Cookie。

use std::net::{IpAddr, SocketAddr};

use axum::Json;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use tpl_core::auth::{AuthError, SessionToken};

use crate::{AppState, SessionCookieSettings};

/// 反向代理追加客户端地址用的请求头。
const X_FORWARDED_FOR: HeaderName = HeaderName::from_static("x-forwarded-for");
/// 请求体上限。登录/注册/管理操作只有几个短字段，16 KiB 绰绰有余。
pub(crate) const MAX_REQUEST_BODY_BYTES: usize = 16 * 1024;
/// Cookie 里令牌的最大长度。我们签发的令牌是 64 个字符，超长的一定是伪造的。
const MAX_TOKEN_CHARS: usize = 256;

/// 统一的错误响应：`{"error": {"code": "...", "message": "..."}}`。
pub(crate) struct ApiError {
    pub(crate) status: StatusCode,
    code: &'static str,
    message: String,
    /// 限流时告诉客户端多少秒后可以重试（`Retry-After` 头）。
    retry_after_secs: Option<u64>,
}

impl ApiError {
    pub(crate) fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            retry_after_secs: None,
        }
    }

    pub(crate) fn auth_unavailable() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "auth_unavailable",
            "认证功能未启用：需要同时配置数据库和 Redis",
        )
    }
}

impl ApiError {
    pub(crate) fn admin_unavailable() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "admin_unavailable",
            "管理后台未启用：需要同时配置数据库和 Redis",
        )
    }

    pub(crate) fn not_found(what: &str) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", format!("{what}不存在"))
    }

    pub(crate) fn invalid_parameter(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_parameter", message)
    }
}

impl From<AuthError> for ApiError {
    fn from(error: AuthError) -> Self {
        match error {
            AuthError::InvalidEmail => {
                Self::new(StatusCode::BAD_REQUEST, "invalid_email", error.to_string())
            }
            AuthError::PasswordTooShort => Self::new(
                StatusCode::BAD_REQUEST,
                "password_too_short",
                error.to_string(),
            ),
            AuthError::PasswordTooLong => Self::new(
                StatusCode::BAD_REQUEST,
                "password_too_long",
                error.to_string(),
            ),
            AuthError::InvalidUsername => Self::new(
                StatusCode::BAD_REQUEST,
                "invalid_username",
                error.to_string(),
            ),
            AuthError::AccountDisabled => {
                Self::new(StatusCode::FORBIDDEN, "account_disabled", error.to_string())
            }
            AuthError::EmailTaken => {
                Self::new(StatusCode::CONFLICT, "email_taken", error.to_string())
            }
            AuthError::InvalidCredentials => Self::new(
                StatusCode::UNAUTHORIZED,
                "invalid_credentials",
                error.to_string(),
            ),
            AuthError::Unauthenticated => Self::new(
                StatusCode::UNAUTHORIZED,
                "unauthenticated",
                error.to_string(),
            ),
            AuthError::TooManyAttempts { retry_after } => Self {
                // 向上取整，且至少 1 秒：告诉客户端的等待时间不能比实际短，否则它会过早重试再被拒。
                retry_after_secs: Some(
                    retry_after
                        .as_secs()
                        .saturating_add(u64::from(retry_after.subsec_nanos() > 0))
                        .max(1),
                ),
                ..Self::new(
                    StatusCode::TOO_MANY_REQUESTS,
                    "too_many_attempts",
                    error.to_string(),
                )
            },
            AuthError::Internal(reason) => {
                // 原因只写日志；响应里不暴露任何内部细节。
                tracing::error!(%reason, "认证用例内部错误");
                Self::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "服务器内部错误",
                )
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "error": { "code": self.code, "message": self.message } });
        let mut response = (self.status, Json(body)).into_response();
        if let Some(retry_after_secs) = self.retry_after_secs {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(retry_after_secs));
        }
        response
    }
}

/// 识别客户端 IP，用于限流。
///
/// 默认只用 TCP 连接的对端地址（客户端无法伪造）。`trust_forwarded_for` 开启时，改用
/// `X-Forwarded-For` 里**最右**的那个地址：它是我们自己的反向代理追加的；左边的条目都是客户端
/// 自己写的，可以随便伪造，绝不能用。最右的值解析不出 IP 时，说明请求没有经过代理，退回对端地址。
pub(crate) fn client_ip(
    app_state: &AppState,
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
) -> Option<IpAddr> {
    let forwarded = app_state
        .trust_forwarded_for
        .then(|| {
            headers
                .get_all(X_FORWARDED_FOR)
                .iter()
                .filter_map(|value| value.to_str().ok())
                .flat_map(|value| value.split(','))
                .map(str::trim)
                .rfind(|entry| !entry.is_empty())
                .and_then(|entry| entry.parse::<IpAddr>().ok())
        })
        .flatten();
    forwarded
        .or_else(|| peer.map(|address| address.ip()))
        .map(|ip| ip.to_canonical())
}

/// 从请求的 Cookie 里取出指定 Cookie 的会话令牌。
pub(crate) fn session_token_from(
    cookie_jar: &CookieJar,
    settings: &SessionCookieSettings,
) -> Option<SessionToken> {
    cookie_jar
        .get(&settings.name)
        .map(Cookie::value)
        .filter(|token_text| !token_text.is_empty() && token_text.len() <= MAX_TOKEN_CHARS)
        .map(SessionToken::new)
}

/// 构造会话 Cookie：HTTP-only、`SameSite=Lax`、整站有效，`Max-Age` 与服务端会话有效期一致。
pub(crate) fn build_session_cookie(
    settings: &SessionCookieSettings,
    session_token: &SessionToken,
    session_ttl: std::time::Duration,
) -> Cookie<'static> {
    let max_age_secs = i64::try_from(session_ttl.as_secs()).unwrap_or(i64::MAX);
    Cookie::build((settings.name.clone(), session_token.as_str().to_owned()))
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(settings.secure)
        .path("/")
        .max_age(time::Duration::seconds(max_age_secs))
        .build()
}

/// 让浏览器删除 Cookie：属性（尤其是 `Path`）必须与设置时一致，否则浏览器不会删。
pub(crate) fn build_removal_cookie(settings: &SessionCookieSettings) -> Cookie<'static> {
    Cookie::build((settings.name.clone(), ""))
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(settings.secure)
        .path("/")
        .max_age(time::Duration::ZERO)
        .build()
}
