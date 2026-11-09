//! HTTP 错误映射：稳定错误码与状态，不向调用方暴露内部细节。

use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use tpl_mcp_core::{AuthError, McpError};

use crate::dto::{ErrorBody, ErrorResponse};

/// API 层错误：状态、稳定错误码与展示用说明。
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    /// 构造错误。
    pub(crate) fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    /// 请求参数不合法。
    pub(crate) fn invalid_parameter(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }

    /// 调用方身份缺失或无效。
    pub(crate) fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", message)
    }
}

impl From<AuthError> for ApiError {
    fn from(error: AuthError) -> Self {
        if matches!(error, AuthError::Configuration) {
            tracing::warn!(error = %error, "MCP 调用方认证配置无效");
            return Self::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "服务内部错误",
            );
        }
        Self::unauthorized(error.to_string())
    }
}

impl From<McpError> for ApiError {
    fn from(error: McpError) -> Self {
        let status = match error {
            McpError::InvalidRequest(_) | McpError::UnsafeEndpoint(_) => StatusCode::BAD_REQUEST,
            McpError::Unauthorized => StatusCode::UNAUTHORIZED,
            McpError::Forbidden => StatusCode::FORBIDDEN,
            McpError::NotFound => StatusCode::NOT_FOUND,
            McpError::Conflict | McpError::CredentialUnavailable => StatusCode::CONFLICT,
            McpError::UpstreamUnavailable => StatusCode::BAD_GATEWAY,
            McpError::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let code = match error {
            McpError::InvalidRequest(_) => "invalid_request",
            McpError::Unauthorized => "unauthorized",
            McpError::Forbidden => "forbidden",
            McpError::NotFound => "not_found",
            McpError::Conflict => "conflict",
            McpError::UnsafeEndpoint(_) => "unsafe_endpoint",
            McpError::CredentialUnavailable => "credential_unavailable",
            McpError::UpstreamUnavailable => "upstream_unavailable",
            McpError::Internal => "internal",
        };
        if status.is_server_error() {
            tracing::warn!(code, "MCP 请求处理失败");
        }
        Self {
            status,
            code,
            message: error.to_string(),
        }
    }
}

impl From<JsonRejection> for ApiError {
    fn from(rejection: JsonRejection) -> Self {
        Self::invalid_parameter(rejection.body_text())
    }
}

impl From<QueryRejection> for ApiError {
    fn from(rejection: QueryRejection) -> Self {
        Self::invalid_parameter(rejection.body_text())
    }
}

impl From<PathRejection> for ApiError {
    fn from(rejection: PathRejection) -> Self {
        Self::invalid_parameter(rejection.body_text())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorResponse {
            error: ErrorBody {
                code: self.code.to_owned(),
                message: self.message,
            },
        };
        (self.status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapped(error: McpError) -> (StatusCode, &'static str) {
        let api_error = ApiError::from(error);
        (api_error.status, api_error.code)
    }

    #[test]
    fn domain_errors_map_to_stable_status_and_code() {
        assert_eq!(
            mapped(McpError::InvalidRequest("字段缺失".to_owned())),
            (StatusCode::BAD_REQUEST, "invalid_request")
        );
        assert_eq!(
            mapped(McpError::UnsafeEndpoint("私网地址".to_owned())),
            (StatusCode::BAD_REQUEST, "unsafe_endpoint")
        );
        assert_eq!(
            mapped(McpError::Unauthorized),
            (StatusCode::UNAUTHORIZED, "unauthorized")
        );
        assert_eq!(
            mapped(McpError::Forbidden),
            (StatusCode::FORBIDDEN, "forbidden")
        );
        assert_eq!(
            mapped(McpError::NotFound),
            (StatusCode::NOT_FOUND, "not_found")
        );
        assert_eq!(
            mapped(McpError::Conflict),
            (StatusCode::CONFLICT, "conflict")
        );
        assert_eq!(
            mapped(McpError::CredentialUnavailable),
            (StatusCode::CONFLICT, "credential_unavailable")
        );
        assert_eq!(
            mapped(McpError::UpstreamUnavailable),
            (StatusCode::BAD_GATEWAY, "upstream_unavailable")
        );
        assert_eq!(
            mapped(McpError::Internal),
            (StatusCode::INTERNAL_SERVER_ERROR, "internal")
        );
    }

    #[test]
    fn authentication_errors_are_unauthorized_without_leaking_configuration() {
        let error = ApiError::from(AuthError::InvalidUserContext);
        assert_eq!(error.status, StatusCode::UNAUTHORIZED);
        assert_eq!(error.code, "unauthorized");

        let configuration = ApiError::from(AuthError::Configuration);
        assert_eq!(configuration.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(configuration.code, "internal");
        assert!(!configuration.message.contains("消费者"));
    }
}
