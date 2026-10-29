//! 稳定、低敏的 tracing HTTP 错误响应映射。

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use tpl_agent_tracing_core::event::EventValidationError;
use tpl_agent_tracing_core::ports::{AdminAuthError, TraceStoreError};

/// API 统一错误结构。
#[derive(Serialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Serialize)]
struct ErrorBody {
    code: &'static str,
    message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_seq: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    conflict_seq: Option<u64>,
}

/// 可转换为稳定 HTTP 状态、错误码和简短说明的 API 错误。
pub(crate) struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
    expected_seq: Option<u64>,
    conflict_seq: Option<u64>,
}

impl ApiError {
    pub(crate) fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message,
            expected_seq: None,
            conflict_seq: None,
        }
    }

    pub(crate) fn invalid_json(status: StatusCode) -> Self {
        Self::new(status, "invalid_json", "请求体不是有效的 JSON 事件批次")
    }

    pub(crate) fn query_invalid() -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_query", "查询参数格式无效")
    }

    pub(crate) fn from_event_validation(error: EventValidationError) -> Self {
        match error {
            EventValidationError::SchemaVersion => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "unsupported_schema_version",
                "事件 schema 版本不受支持",
            ),
            EventValidationError::BatchSize | EventValidationError::EventTooLarge => Self::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "ingest_limit_exceeded",
                "事件批次超过大小或条数上限",
            ),
            EventValidationError::SensitiveField => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "sensitive_field_forbidden",
                "事件包含禁止的敏感字段",
            ),
            EventValidationError::RunId
            | EventValidationError::Sequence
            | EventValidationError::EventType
            | EventValidationError::Payload
            | EventValidationError::TraceContext => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "event_schema_invalid",
                "事件 envelope 或 payload 不符合 schema",
            ),
        }
    }

    pub(crate) fn from_store(error: TraceStoreError) -> Self {
        match error {
            TraceStoreError::ProducerUnauthorized => Self::new(
                StatusCode::UNAUTHORIZED,
                "producer_unauthorized",
                "producer 凭据无效",
            ),
            TraceStoreError::NotFound => {
                Self::new(StatusCode::NOT_FOUND, "not_found", "轨迹记录不存在")
            }
            TraceStoreError::SequenceGap { expected_seq } => Self {
                expected_seq: Some(expected_seq),
                ..Self::new(
                    StatusCode::CONFLICT,
                    "sequence_gap",
                    "事件序号存在缺口，请从 expected_seq 继续发送",
                )
            },
            TraceStoreError::SequenceConflict { seq } => Self {
                conflict_seq: Some(seq),
                ..Self::new(
                    StatusCode::CONFLICT,
                    "sequence_conflict",
                    "该序号已存在不同事件内容",
                )
            },
            TraceStoreError::RunStateConflict => Self::new(
                StatusCode::CONFLICT,
                "run_state_conflict",
                "Run 生命周期状态迁移不合法",
            ),
            TraceStoreError::RunTerminal => Self::new(
                StatusCode::CONFLICT,
                "run_terminal",
                "Run 已进入终态，不能继续追加事件",
            ),
            TraceStoreError::TraceIdConflict => Self::new(
                StatusCode::CONFLICT,
                "trace_id_conflict",
                "同一 Run 的 trace_id 必须保持一致",
            ),
            TraceStoreError::ThreadSourceConflict => Self::new(
                StatusCode::CONFLICT,
                "thread_source_conflict",
                "thread_id 已由另一个 producer source 使用",
            ),
            TraceStoreError::ExportNotReady => Self::new(
                StatusCode::CONFLICT,
                "export_not_ready",
                "Run 尚未终结或事件序列不完整",
            ),
            TraceStoreError::ExportTooLarge => Self::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "export_too_large",
                "事件导出超过条数或文件大小上限",
            ),
            TraceStoreError::StorageUnavailable => {
                tracing::error!("tracing 存储操作失败");
                Self::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "服务器内部错误",
                )
            }
        }
    }

    pub(crate) fn from_admin_auth(error: AdminAuthError) -> Self {
        match error {
            AdminAuthError::Unauthenticated => Self::new(
                StatusCode::UNAUTHORIZED,
                "admin_unauthenticated",
                "需要有效的管理员会话",
            ),
            AdminAuthError::Unavailable => Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "admin_auth_unavailable",
                "管理员认证服务暂不可用",
            ),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorEnvelope {
                error: ErrorBody {
                    code: self.code,
                    message: self.message,
                    expected_seq: self.expected_seq,
                    conflict_seq: self.conflict_seq,
                },
            }),
        )
            .into_response()
    }
}
