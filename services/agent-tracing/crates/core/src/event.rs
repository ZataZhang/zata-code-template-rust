//! Agent Run 事件 envelope、目录与写入边界校验。

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;

/// 接入契约支持的 schema 版本。
pub const EVENT_SCHEMA_VERSION: u16 = 1;
/// 一个请求最多包含的事件条数。
pub const MAX_BATCH_EVENTS: usize = 100;
/// 单条序列化事件的最大字节数。
pub const MAX_EVENT_BYTES: usize = 256 * 1024;
/// 服务端允许的最大连续序号。
pub const MAX_EVENT_SEQUENCE: u64 = i64::MAX as u64;

/// 有界 Run 事件批次。
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EventBatch {
    /// 事件契约版本。
    pub schema_version: u16,
    /// 同一 Run 内按序排列的事件。
    pub events: Vec<EventEnvelope>,
}

/// 语言无关的 canonical Agent Run 事件。
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EventEnvelope {
    /// Run 内从 1 开始的连续序号。
    pub seq: u64,
    /// 版本化事件目录中的事件名。
    pub event_type: String,
    /// 事件发生时间，必须带时区。
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    /// OpenTelemetry trace ID；缺省时读取投影会稳定派生。
    #[serde(default)]
    pub trace_id: Option<String>,
    /// OpenTelemetry span ID；缺省时仅对可识别的标准事件派生。
    #[serde(default)]
    pub span_id: Option<String>,
    /// 已由 producer 明确确认的父 span ID。
    #[serde(default)]
    pub parent_span_id: Option<String>,
    /// 必须符合对应标准事件的冻结载荷字段。
    pub payload: Value,
}

/// 事件契约校验错误。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EventValidationError {
    /// schema 版本不受支持。
    #[error("schema_version_invalid")]
    SchemaVersion,
    /// 事件批次为空或超出条数上限。
    #[error("event_batch_size_invalid")]
    BatchSize,
    /// Run 标识无效。
    #[error("run_id_invalid")]
    RunId,
    /// 事件序号非连续或超出数据库整数范围。
    #[error("event_sequence_invalid")]
    Sequence,
    /// 事件类型名格式无效。
    #[error("event_type_invalid")]
    EventType,
    /// 事件 payload 不符合冻结目录。
    #[error("event_payload_invalid")]
    Payload,
    /// trace/span 标识格式错误或父级关系缺少 span。
    #[error("trace_context_invalid")]
    TraceContext,
    /// 事件或 payload 含禁止的敏感字段名。
    #[error("sensitive_field_forbidden")]
    SensitiveField,
    /// 序列化事件超过单事件上限。
    #[error("event_too_large")]
    EventTooLarge,
}

const SENSITIVE_KEY_PARTS: [&str; 7] = [
    "api_key",
    "authorization",
    "cookie",
    "password",
    "reasoning",
    "secret",
    "token",
];

const EVENT_SCHEMAS: &[(&str, &[&str], &[&str])] = &[
    (
        "run.created",
        &[
            "thread_id",
            "agent_id",
            "agent_snapshot_checksum",
            "context_pack_checksum",
        ],
        &[],
    ),
    (
        "input.accepted",
        &["input_id", "content", "content_checksum", "resources"],
        &[],
    ),
    ("run.started", &["started_at"], &[]),
    ("message.started", &["message_id", "role"], &[]),
    (
        "message.delta",
        &["message_id", "content_index", "delta"],
        &[],
    ),
    (
        "message.completed",
        &["message_id", "content", "content_checksum"],
        &[],
    ),
    (
        "model.call.started",
        &["model_call_id", "model_name", "turn_index"],
        &[],
    ),
    (
        "model.call.completed",
        &[
            "model_call_id",
            "model_name",
            "turn_index",
            "usage",
            "finish_reason",
        ],
        &[],
    ),
    (
        "model.call.failed",
        &["model_call_id", "model_name", "turn_index", "error"],
        &[],
    ),
    (
        "tool.call.started",
        &["tool_call_id", "tool_name", "arguments"],
        &["parent_model_call_id"],
    ),
    (
        "tool.call.completed",
        &["tool_call_id", "result", "result_checksum"],
        &[],
    ),
    ("tool.call.failed", &["tool_call_id", "error"], &[]),
    (
        "delegation.started",
        &[
            "delegation_id",
            "child_run_id",
            "child_agent_id",
            "child_agent_snapshot_checksum",
            "child_context_pack_checksum",
            "budget_limit",
            "failure_policy",
            "origin",
        ],
        &[],
    ),
    (
        "delegation.completed",
        &[
            "delegation_id",
            "child_run_id",
            "status",
            "result_checksum",
            "failure_policy",
        ],
        &[],
    ),
    (
        "delegation.failed",
        &[
            "delegation_id",
            "child_run_id",
            "error_code",
            "failure_policy",
        ],
        &[],
    ),
    (
        "delegation.cancelled",
        &["delegation_id", "child_run_id", "failure_policy"],
        &[],
    ),
    (
        "delegation.partial_completed",
        &[
            "delegation_id",
            "child_run_id",
            "failure_policy",
            "missing_child_run_ids",
            "completed_child_run_ids",
        ],
        &[],
    ),
    (
        "delegation.rejected",
        &["reason_code", "tool_call_id_hash"],
        &[],
    ),
    (
        "delegation.unavailable",
        &["reason_code", "tool_call_id_hash"],
        &[],
    ),
    (
        "delegation.lineage_admitted",
        &[
            "delegation_id",
            "child_run_id",
            "child_agent_id",
            "external_child_id_hash",
        ],
        &[],
    ),
    (
        "delegation.lineage_rejected",
        &["reason_code", "external_child_id_hash"],
        &[],
    ),
    (
        "routing.selected",
        &[
            "entry_source",
            "entry_agent_id",
            "candidate_agent_ids",
            "selected_agent_id",
            "reason_code",
            "policy_checksum",
            "capability_checksum",
        ],
        &[],
    ),
    (
        "routing.router_invoked",
        &[
            "model_call_id",
            "model_name",
            "usage",
            "usage_kind",
            "candidate_count",
        ],
        &[],
    ),
    (
        "artifact.created",
        &[
            "type",
            "artifact_id",
            "filename",
            "content_type",
            "size",
            "checksum",
        ],
        &["sync_action"],
    ),
    (
        "decision.gate.evaluated",
        &[
            "decision_id",
            "gate",
            "tool_name",
            "tool_call_ref",
            "verdict",
            "answer",
            "probability",
            "probabilities",
            "irreversible",
            "fail_open",
            "policy_fingerprint",
            "elapsed_ms",
            "judgement_state",
        ],
        &[],
    ),
    ("run.cancelling", &["requested_by", "requested_at"], &[]),
    ("run.completed", &["message_id"], &[]),
    ("run.failed", &["error"], &[]),
    ("run.cancelled", &["external_stop_confirmed"], &[]),
    ("run.interrupted", &["reason_code", "lease_expired_at"], &[]),
];

/// 校验一批事件的版本、序号、边界、敏感键与标准 payload schema。
///
/// # Errors
///
/// 任一字段超限、序号非连续或 payload 不符合目录时返回稳定的校验错误。
pub fn validate_batch(run_id: &str, batch: &EventBatch) -> Result<(), EventValidationError> {
    let valid_run_id =
        !run_id.trim().is_empty() && run_id.len() <= 256 && !run_id.chars().any(char::is_control);
    if !valid_run_id {
        return Err(EventValidationError::RunId);
    }
    if batch.schema_version != EVENT_SCHEMA_VERSION {
        return Err(EventValidationError::SchemaVersion);
    }
    if batch.events.is_empty() || batch.events.len() > MAX_BATCH_EVENTS {
        return Err(EventValidationError::BatchSize);
    }
    let first_sequence = batch.events[0].seq;
    if first_sequence == 0 || first_sequence > MAX_EVENT_SEQUENCE {
        return Err(EventValidationError::Sequence);
    }
    for (event_offset, event) in batch.events.iter().enumerate() {
        let expected_sequence = first_sequence.saturating_add(event_offset as u64);
        if event.seq != expected_sequence || event.seq > MAX_EVENT_SEQUENCE {
            return Err(EventValidationError::Sequence);
        }
        validate_event(event)?;
        let serialized_event =
            serde_json::to_vec(event).map_err(|_| EventValidationError::Payload)?;
        if serialized_event.len() > MAX_EVENT_BYTES {
            return Err(EventValidationError::EventTooLarge);
        }
    }
    Ok(())
}

/// 校验单条事件的 envelope、标准字段与递归敏感键。
///
/// # Errors
///
/// 事件格式、标识、payload 或敏感字段不符合契约时返回稳定错误。
pub fn validate_event(event: &EventEnvelope) -> Result<(), EventValidationError> {
    let valid_event_type = !event.event_type.is_empty()
        && event.event_type.len() <= 128
        && event
            .event_type
            .chars()
            .enumerate()
            .all(|(index, character)| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || (index > 0 && matches!(character, '.' | '_' | '-'))
            });
    if !valid_event_type {
        return Err(EventValidationError::EventType);
    }
    if !is_valid_trace_id(event.trace_id.as_deref())
        || !is_valid_span_id(event.span_id.as_deref())
        || !is_valid_span_id(event.parent_span_id.as_deref())
        || (event.span_id.is_some() && event.trace_id.is_none())
        || (event.parent_span_id.is_some() && event.span_id.is_none())
    {
        return Err(EventValidationError::TraceContext);
    }
    let contains_sensitive_payload_key = contains_sensitive_key(&event.payload);
    if contains_sensitive_payload_key {
        return Err(EventValidationError::SensitiveField);
    }
    let Some((_, required_keys, optional_keys)) = EVENT_SCHEMAS
        .iter()
        .find(|(event_type, _, _)| *event_type == event.event_type)
    else {
        return Err(EventValidationError::EventType);
    };
    let Some(payload_object) = event.payload.as_object() else {
        return Err(EventValidationError::Payload);
    };
    let payload_keys = payload_object
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let required_keys_present = required_keys.iter().all(|key| payload_keys.contains(key));
    let allowed_keys = required_keys
        .iter()
        .chain(optional_keys.iter())
        .copied()
        .collect::<BTreeSet<_>>();
    let no_unknown_keys = payload_keys.iter().all(|key| allowed_keys.contains(key));
    if !required_keys_present || !no_unknown_keys {
        return Err(EventValidationError::Payload);
    }
    Ok(())
}

/// 递归检查 JSON 对象键名是否含凭据或隐藏推理字段。
pub fn contains_sensitive_key(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, nested_value)| {
            let lowercase_key = key.to_ascii_lowercase();
            SENSITIVE_KEY_PARTS
                .iter()
                .any(|sensitive_part| lowercase_key.contains(sensitive_part))
                || contains_sensitive_key(nested_value)
        }),
        Value::Array(values) => values.iter().any(contains_sensitive_key),
        _ => false,
    }
}

/// 判断 trace ID 是否为 32 位小写十六进制且非全零。
pub fn is_valid_trace_id(identifier: Option<&str>) -> bool {
    identifier.is_none_or(|value| {
        value.len() == 32
            && value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            && value.bytes().any(|byte| byte != b'0')
    })
}

/// 判断 span ID 是否为 16 位小写十六进制且非全零。
pub fn is_valid_span_id(identifier: Option<&str>) -> bool {
    identifier.is_none_or(|value| {
        value.len() == 16
            && value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            && value.bytes().any(|byte| byte != b'0')
    })
}
