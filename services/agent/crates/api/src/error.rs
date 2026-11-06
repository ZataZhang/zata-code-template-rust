//! HTTP 错误映射：稳定错误码与状态，不向调用方暴露内部细节。

use axum::Json;
use axum::extract::multipart::MultipartRejection;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use tpl_agent_core::error::AgentError;

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
}

impl From<AgentError> for ApiError {
    fn from(error: AgentError) -> Self {
        let status = match error {
            AgentError::InvalidRequest(_) => StatusCode::BAD_REQUEST,
            AgentError::ServiceUnauthorized | AgentError::Unauthenticated => {
                StatusCode::UNAUTHORIZED
            }
            AgentError::Forbidden(_) => StatusCode::FORBIDDEN,
            AgentError::NotFound(_) => StatusCode::NOT_FOUND,
            AgentError::Conflict(_)
            | AgentError::IdempotencyConflict
            | AgentError::RuntimeBindingImmutable
            | AgentError::RuntimeUnavailable
            | AgentError::AgentNotRunnable
            | AgentError::NoRunnableAgent
            | AgentError::RunCancelled => StatusCode::CONFLICT,
            AgentError::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            AgentError::UnsupportedMediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            AgentError::Execution(_) | AgentError::Upstream(_) => StatusCode::BAD_GATEWAY,
            AgentError::IdentityUnavailable
            | AgentError::UpstreamUnavailable
            | AgentError::StorageUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            AgentError::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let code = error.code();
        if status.is_server_error() {
            tracing::warn!(code, "Agent 请求处理失败");
        }
        let message = match &error {
            AgentError::InvalidRequest(message) => message.clone(),
            _ => error.to_string(),
        };
        Self {
            status,
            code,
            message,
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

impl From<MultipartRejection> for ApiError {
    fn from(rejection: MultipartRejection) -> Self {
        if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
            return Self::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload_too_large",
                "请求体超过上限",
            );
        }
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
