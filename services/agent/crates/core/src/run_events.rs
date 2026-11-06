//! 构造符合 tracing 冻结目录的脱敏 Run 事件。
//!
//! 所有自由文本（输入、消息、工具参数/结果、产物文件名）在事件里替换为占位符，
//! 只保留摘要与低敏关联 ID；完整事实保存在 Session 消息与 Run 快照中。

use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::agent_catalog::BusinessAgent;
use crate::error::AgentError;
use crate::hash::{sha256_hex, sha256_json};
use crate::resources::{Artifact, ResourceSnapshot};
use crate::routing::RoutingDecision;
use crate::run::{RunEventDraft, RunStatus};
use crate::session::MessageRole;
use crate::time_utils::rfc3339;

/// 事件中替代自由文本的占位符。
pub const REDACTED_TEXT: &str = "[已脱敏]";

fn draft(event_type: &str, occurred_at: OffsetDateTime, payload: Value) -> RunEventDraft {
    RunEventDraft::new(event_type, occurred_at, payload)
}

/// `run.created`：记录 thread、Agent 快照与上下文摘要。
pub fn run_created(
    occurred_at: OffsetDateTime,
    thread_id: Uuid,
    agent: &BusinessAgent,
    context_pack_checksum: &str,
) -> RunEventDraft {
    draft(
        "run.created",
        occurred_at,
        json!({
            "thread_id": thread_id.to_string(),
            "agent_id": agent.id.to_string(),
            "agent_snapshot_checksum": agent.descriptor_checksum,
            "context_pack_checksum": context_pack_checksum,
        }),
    )
}

/// `input.accepted`：记录输入消息、正文摘要与资源元数据。
pub fn input_accepted(
    occurred_at: OffsetDateTime,
    input_message_id: Uuid,
    content: &str,
    resources: &[ResourceSnapshot],
) -> RunEventDraft {
    let resources = resources
        .iter()
        .map(|resource| {
            json!({
                "resource_id": resource.resource_id.to_string(),
                "media_type": resource.media_type,
                "size": resource.size_bytes,
                "sha256": resource.sha256,
            })
        })
        .collect::<Vec<_>>();
    draft(
        "input.accepted",
        occurred_at,
        json!({
            "input_id": input_message_id.to_string(),
            "content": REDACTED_TEXT,
            "content_checksum": sha256_hex(content.as_bytes()),
            "resources": resources,
        }),
    )
}

/// `routing.selected`：记录入口来源、候选和选择结果。
pub fn routing_selected(occurred_at: OffsetDateTime, decision: &RoutingDecision) -> RunEventDraft {
    draft("routing.selected", occurred_at, decision.event_payload())
}

/// `run.started`：记录开始时间。
pub fn run_started(occurred_at: OffsetDateTime) -> RunEventDraft {
    draft(
        "run.started",
        occurred_at,
        json!({"started_at": rfc3339(occurred_at)}),
    )
}

/// `message.started`：记录 assistant 消息开始。
pub fn message_started(
    occurred_at: OffsetDateTime,
    message_id: Uuid,
    role: MessageRole,
) -> RunEventDraft {
    draft(
        "message.started",
        occurred_at,
        json!({"message_id": message_id.to_string(), "role": role.as_str()}),
    )
}

/// `message.delta`：记录脱敏增量位置。
pub fn message_delta(
    occurred_at: OffsetDateTime,
    message_id: Uuid,
    content_index: u32,
) -> RunEventDraft {
    draft(
        "message.delta",
        occurred_at,
        json!({
            "message_id": message_id.to_string(),
            "content_index": content_index,
            "delta": REDACTED_TEXT,
        }),
    )
}

/// `message.completed`：记录完整消息摘要。
pub fn message_completed(
    occurred_at: OffsetDateTime,
    message_id: Uuid,
    content: &str,
) -> RunEventDraft {
    draft(
        "message.completed",
        occurred_at,
        json!({
            "message_id": message_id.to_string(),
            "content": REDACTED_TEXT,
            "content_checksum": sha256_hex(content.as_bytes()),
        }),
    )
}

/// `tool.call.started`：记录工具名，不记录参数。
pub fn tool_call_started(
    occurred_at: OffsetDateTime,
    tool_call_id: &str,
    tool_name: &str,
) -> RunEventDraft {
    draft(
        "tool.call.started",
        occurred_at,
        json!({
            "tool_call_id": tool_call_id,
            "tool_name": tool_name,
            "arguments": REDACTED_TEXT,
        }),
    )
}

/// `tool.call.completed`：记录结果摘要，不记录结果正文。
///
/// # Errors
///
/// 结果无法序列化时返回内部错误。
pub fn tool_call_completed(
    occurred_at: OffsetDateTime,
    tool_call_id: &str,
    result: &Value,
) -> Result<RunEventDraft, AgentError> {
    Ok(draft(
        "tool.call.completed",
        occurred_at,
        json!({
            "tool_call_id": tool_call_id,
            "result": REDACTED_TEXT,
            "result_checksum": sha256_json(result)?,
        }),
    ))
}

/// `tool.call.failed`：记录低敏错误类别。
pub fn tool_call_failed(
    occurred_at: OffsetDateTime,
    tool_call_id: &str,
    error_code: &str,
) -> RunEventDraft {
    draft(
        "tool.call.failed",
        occurred_at,
        json!({"tool_call_id": tool_call_id, "error": error_code}),
    )
}

/// `artifact.created`：记录产物摘要，不记录文件名。
pub fn artifact_created(occurred_at: OffsetDateTime, artifact: &Artifact) -> RunEventDraft {
    draft(
        "artifact.created",
        occurred_at,
        json!({
            "type": "file",
            "artifact_id": artifact.id.to_string(),
            "filename": REDACTED_TEXT,
            "content_type": artifact.media_type,
            "size": artifact.size_bytes,
            "checksum": artifact.sha256,
        }),
    )
}

/// `delegation.started`：记录子 Run 与子 Agent 快照。
#[allow(clippy::too_many_arguments)]
pub fn delegation_started(
    occurred_at: OffsetDateTime,
    delegation_id: Uuid,
    child_run_id: Uuid,
    child_agent_id: Uuid,
    child_agent_snapshot_checksum: &str,
    child_context_pack_checksum: &str,
    budget_limit: u8,
    failure_policy: &str,
) -> RunEventDraft {
    draft(
        "delegation.started",
        occurred_at,
        json!({
            "delegation_id": delegation_id.to_string(),
            "child_run_id": child_run_id.to_string(),
            "child_agent_id": child_agent_id.to_string(),
            "child_agent_snapshot_checksum": child_agent_snapshot_checksum,
            "child_context_pack_checksum": child_context_pack_checksum,
            "budget_limit": budget_limit,
            "failure_policy": failure_policy,
            "origin": "delegation",
        }),
    )
}

/// `delegation.completed`：记录子 Run 终态与结果摘要。
pub fn delegation_completed(
    occurred_at: OffsetDateTime,
    delegation_id: Uuid,
    child_run_id: Uuid,
    status: RunStatus,
    result_checksum: &str,
    failure_policy: &str,
) -> RunEventDraft {
    draft(
        "delegation.completed",
        occurred_at,
        json!({
            "delegation_id": delegation_id.to_string(),
            "child_run_id": child_run_id.to_string(),
            "status": status.as_str(),
            "result_checksum": result_checksum,
            "failure_policy": failure_policy,
        }),
    )
}

/// `delegation.failed`：记录子 Run 失败类别。
pub fn delegation_failed(
    occurred_at: OffsetDateTime,
    delegation_id: Uuid,
    child_run_id: Uuid,
    error_code: &str,
    failure_policy: &str,
) -> RunEventDraft {
    draft(
        "delegation.failed",
        occurred_at,
        json!({
            "delegation_id": delegation_id.to_string(),
            "child_run_id": child_run_id.to_string(),
            "error_code": error_code,
            "failure_policy": failure_policy,
        }),
    )
}

/// `delegation.cancelled`：记录子 Run 取消。
pub fn delegation_cancelled(
    occurred_at: OffsetDateTime,
    delegation_id: Uuid,
    child_run_id: Uuid,
    failure_policy: &str,
) -> RunEventDraft {
    draft(
        "delegation.cancelled",
        occurred_at,
        json!({
            "delegation_id": delegation_id.to_string(),
            "child_run_id": child_run_id.to_string(),
            "failure_policy": failure_policy,
        }),
    )
}

/// `delegation.rejected`：记录策略拒绝原因与工具调用摘要。
pub fn delegation_rejected(
    occurred_at: OffsetDateTime,
    reason_code: &str,
    tool_call_id: &str,
) -> RunEventDraft {
    draft(
        "delegation.rejected",
        occurred_at,
        json!({
            "reason_code": reason_code,
            "tool_call_id_hash": sha256_hex(tool_call_id.as_bytes()),
        }),
    )
}

/// `run.cancelling`：记录取消请求者与时间。
pub fn run_cancelling(
    occurred_at: OffsetDateTime,
    requested_by: &str,
    requested_at: OffsetDateTime,
) -> RunEventDraft {
    draft(
        "run.cancelling",
        occurred_at,
        json!({
            "requested_by": requested_by,
            "requested_at": rfc3339(requested_at),
        }),
    )
}

/// `run.completed`：记录成功消息。
pub fn run_completed(occurred_at: OffsetDateTime, message_id: Uuid) -> RunEventDraft {
    draft(
        "run.completed",
        occurred_at,
        json!({"message_id": message_id.to_string()}),
    )
}

/// `run.failed`：记录低敏错误类别。
pub fn run_failed(occurred_at: OffsetDateTime, error_code: &str) -> RunEventDraft {
    draft("run.failed", occurred_at, json!({"error": error_code}))
}

/// `run.cancelled`：记录是否确认停止外部执行。
pub fn run_cancelled(occurred_at: OffsetDateTime, external_stop_confirmed: bool) -> RunEventDraft {
    draft(
        "run.cancelled",
        occurred_at,
        json!({"external_stop_confirmed": external_stop_confirmed}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_contract::validate_event;
    use crate::time_utils::now;

    #[test]
    fn input_and_message_payloads_are_redacted_with_real_checksums() {
        let run_id = Uuid::new_v4();
        let input = input_accepted(now(), Uuid::new_v4(), "secret prompt", &[]);
        assert_eq!(input.payload["content"], REDACTED_TEXT);
        assert_eq!(
            input.payload["content_checksum"],
            sha256_hex(b"secret prompt")
        );
        assert!(validate_event(run_id, &input).is_ok());

        let completed = message_completed(now(), Uuid::new_v4(), "answer");
        assert_eq!(completed.payload["content"], REDACTED_TEXT);
        assert!(validate_event(run_id, &completed).is_ok());
    }

    #[test]
    fn tool_payloads_do_not_leak_arguments_or_results() {
        let run_id = Uuid::new_v4();
        let started = tool_call_started(now(), "call-1", "text.summarize");
        assert_eq!(started.payload["arguments"], REDACTED_TEXT);
        let completed = tool_call_completed(
            now(),
            "call-1",
            &json!({"summary": "secret", "api_key": "leak"}),
        )
        .unwrap();
        assert_eq!(completed.payload["result"], REDACTED_TEXT);
        assert_eq!(
            completed.payload["result_checksum"].as_str().unwrap().len(),
            64
        );
        assert!(validate_event(run_id, &started).is_ok());
        assert!(validate_event(run_id, &completed).is_ok());
    }

    #[test]
    fn lifecycle_payloads_match_schema() {
        let run_id = Uuid::new_v4();
        let message_id = Uuid::new_v4();
        let events = vec![
            run_started(now()),
            run_cancelling(now(), "owner", now()),
            run_completed(now(), message_id),
            run_failed(now(), "runtime_unavailable"),
            run_cancelled(now(), false),
        ];
        for event in events {
            assert!(
                validate_event(run_id, &event).is_ok(),
                "{}",
                event.event_type
            );
        }
    }
}
