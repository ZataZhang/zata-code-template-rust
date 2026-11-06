//! Agent 服务实际发出的 tracing 事件子集与冻结 schema 校验。
//!
//! 事件只包含低敏关联 ID、状态、时间与摘要；完整提示词、用户资源正文、
//! 工具参数/结果和凭据不进入事件或 outbox。

use std::collections::BTreeSet;

use serde_json::Value;
use uuid::Uuid;

use crate::error::AgentError;
use crate::run::{RunEvent, RunEventDraft};

/// tracing producer 单批最大事件条数。
pub const MAX_TRACE_BATCH_EVENTS: usize = 100;
/// tracing producer 单条序列化事件最大字节数。
pub const MAX_TRACE_EVENT_BYTES: usize = 256 * 1024;

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
        "tool.call.started",
        &["tool_call_id", "tool_name", "arguments"],
        &[],
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
        "delegation.rejected",
        &["reason_code", "tool_call_id_hash"],
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
    ("run.cancelling", &["requested_by", "requested_at"], &[]),
    ("run.completed", &["message_id"], &[]),
    ("run.failed", &["error"], &[]),
    ("run.cancelled", &["external_stop_confirmed"], &[]),
];

/// 校验单条待写入事件。
///
/// # Errors
///
/// 事件类型不在目录、载荷字段不匹配、含敏感键或超长时返回内部错误。
pub fn validate_event(_run_id: Uuid, draft: &RunEventDraft) -> Result<(), AgentError> {
    if !is_valid_event_type(&draft.event_type) {
        return Err(AgentError::Internal);
    }
    validate_payload(&draft.event_type, &draft.payload)?;
    let serialized_payload =
        serde_json::to_vec(&draft.payload).map_err(|_| AgentError::Internal)?;
    if serialized_payload.len() > MAX_TRACE_EVENT_BYTES {
        return Err(AgentError::Internal);
    }
    Ok(())
}

/// 校验一批已编号事件是否连续、在批次上限内并符合 schema。
///
/// # Errors
///
/// 批次数、序号连续性或任一事件不合规时返回内部错误。
pub fn validate_batch(events: &[RunEvent]) -> Result<(), AgentError> {
    if events.is_empty() || events.len() > MAX_TRACE_BATCH_EVENTS {
        return Err(AgentError::Internal);
    }
    let first_sequence = events[0].seq;
    if first_sequence < 0 {
        return Err(AgentError::Internal);
    }
    for (offset, event) in events.iter().enumerate() {
        let expected = first_sequence + i64::try_from(offset).map_err(|_| AgentError::Internal)?;
        if event.seq != expected {
            return Err(AgentError::Internal);
        }
        validate_payload(&event.event_type, &event.payload)?;
        let envelope = serde_json::json!({
            "seq": u64::try_from(event.seq).map_err(|_| AgentError::Internal)?,
            "event_type": event.event_type,
            "occurred_at": crate::time_utils::rfc3339(event.occurred_at),
            "payload": event.payload,
        });
        let serialized = serde_json::to_vec(&envelope).map_err(|_| AgentError::Internal)?;
        if serialized.len() > MAX_TRACE_EVENT_BYTES {
            return Err(AgentError::Internal);
        }
    }
    Ok(())
}

/// 校验事件类型是否符合小写点分格式。
pub fn is_valid_event_type(event_type: &str) -> bool {
    !event_type.is_empty()
        && event_type.len() <= 128
        && event_type.chars().enumerate().all(|(index, character)| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || (index > 0 && matches!(character, '.' | '_' | '-'))
        })
}

/// 校验载荷字段、敏感键与摘要格式。
///
/// # Errors
///
/// 字段缺失/多余、含敏感键或摘要格式错误时返回内部错误。
pub fn validate_payload(event_type: &str, payload: &Value) -> Result<(), AgentError> {
    if contains_sensitive_key(payload) {
        return Err(AgentError::Internal);
    }
    let Some((_, required_keys, optional_keys)) = EVENT_SCHEMAS
        .iter()
        .find(|(known_type, _, _)| *known_type == event_type)
    else {
        return Err(AgentError::Internal);
    };
    let Some(payload_object) = payload.as_object() else {
        return Err(AgentError::Internal);
    };
    let payload_keys = payload_object
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let required_present = required_keys.iter().all(|key| payload_keys.contains(key));
    let allowed_keys = required_keys
        .iter()
        .chain(optional_keys.iter())
        .copied()
        .collect::<BTreeSet<_>>();
    let no_unknown_keys = payload_keys.iter().all(|key| allowed_keys.contains(key));
    if !required_present || !no_unknown_keys {
        return Err(AgentError::Internal);
    }
    for key in [
        "agent_snapshot_checksum",
        "context_pack_checksum",
        "child_agent_snapshot_checksum",
        "child_context_pack_checksum",
        "content_checksum",
        "result_checksum",
        "checksum",
        "policy_checksum",
        "capability_checksum",
        "tool_call_id_hash",
    ] {
        if let Some(value) = payload_object.get(key) {
            let is_checksum = value
                .as_str()
                .is_some_and(|text| text.len() == 64 && text.bytes().all(is_lower_hex));
            if !is_checksum {
                return Err(AgentError::Internal);
            }
        }
    }
    Ok(())
}

/// 递归检查 JSON 对象键是否包含凭据或隐藏推理字段。
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

fn is_lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time_utils::now;

    fn draft(event_type: &str, payload: Value) -> RunEventDraft {
        RunEventDraft::new(event_type, now(), payload)
    }

    #[test]
    fn redacted_lifecycle_payloads_validate() {
        let run_id = Uuid::new_v4();
        let checksum = "a".repeat(64);
        assert!(
            validate_event(
                run_id,
                &draft(
                    "run.created",
                    serde_json::json!({
                        "thread_id": Uuid::new_v4().to_string(),
                        "agent_id": Uuid::new_v4().to_string(),
                        "agent_snapshot_checksum": checksum,
                        "context_pack_checksum": "b".repeat(64),
                    })
                )
            )
            .is_ok()
        );
    }

    #[test]
    fn unknown_keys_and_missing_keys_are_rejected() {
        let run_id = Uuid::new_v4();
        assert!(
            validate_event(
                run_id,
                &draft("run.completed", serde_json::json!({"message_id": "m"}))
            )
            .is_ok()
        );
        assert!(
            validate_event(
                run_id,
                &draft(
                    "run.completed",
                    serde_json::json!({"message_id": "m", "prompt": "secret"})
                )
            )
            .is_err()
        );
        assert!(validate_event(run_id, &draft("run.completed", serde_json::json!({}))).is_err());
    }

    #[test]
    fn sensitive_keys_are_rejected_recursively() {
        assert!(contains_sensitive_key(&serde_json::json!({
            "nested": [{"access_token": "x"}]
        })));
        assert!(!contains_sensitive_key(&serde_json::json!({
            "resources": [{"resource_id": "x", "sha256": "a".repeat(64)}]
        })));
    }

    #[test]
    fn checksum_fields_must_be_lower_hex() {
        assert!(
            validate_payload(
                "run.started",
                &serde_json::json!({"started_at": "2026-10-30T00:00:00Z"})
            )
            .is_ok()
        );
        assert!(
            validate_payload(
                "message.completed",
                &serde_json::json!({
                    "message_id": "m",
                    "content": "[已脱敏]",
                    "content_checksum": "NOT-A-CHECKSUM",
                })
            )
            .is_err()
        );
    }
}
