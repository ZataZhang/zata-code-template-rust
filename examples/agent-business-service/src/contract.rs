//! Agent HTTP 契约中业务服务实际使用的 Run 响应类型。
//!
//! 字段与 `frontends/public/lib/api/agent.ts` 的手写客户端类型一致；未声明的
//! 服务端字段通过 `extra` 原样保留，避免示例服务在转发时丢失契约内容。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// 发送消息请求；与 OpenAPI 的 `SendMessageRequest` 对应。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SendMessageRequest {
    /// 显式选择的 Agent；为空表示使用 Auto 入口。
    pub agent_id: Option<String>,
    /// 用户消息正文。
    pub content: String,
    /// 引用的用户资源。
    #[serde(default)]
    pub resource_ids: Vec<String>,
    /// 选择的 Skill。
    #[serde(default)]
    pub skill_ids: Vec<String>,
}

/// Run 的持久化记录（业务服务关心的字段 + 原样保留的其余字段）。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentRun {
    /// Run UUID。
    pub run_id: String,
    /// 所属组织。
    pub organization_id: String,
    /// 所属 Session。
    pub session_id: String,
    /// 所有者用户 ID。
    pub owner_user_id: i64,
    /// 使用的 Agent。
    pub agent_id: String,
    /// 绑定的 Runtime。
    pub runtime_connection_id: String,
    /// Run 来源。
    pub origin: String,
    /// 当前状态。
    pub status: String,
    /// 幂等键。
    pub idempotency_key: String,
    /// 规范化请求摘要。
    pub request_digest: String,
    /// 已写入的最大事件序号。
    pub last_event_seq: i64,
    /// tracing 投递检查点。
    pub trace_delivered_through_seq: i64,
    /// 失败或取消的低敏错误类别。
    pub error_code: Option<String>,
    /// 创建时间。
    pub created_at: String,
    /// 开始执行时间。
    pub started_at: Option<String>,
    /// 进入终态时间。
    pub finished_at: Option<String>,
    /// 冻结的执行快照。
    pub snapshot: RunSnapshot,
    /// 服务端保留、业务服务不解释的字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 冻结的执行快照（只声明业务入口需要的部分）。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RunSnapshot {
    /// Agent 与 Runtime 绑定。
    pub agent: AgentSnapshot,
    /// 输入快照。
    pub input: InputSnapshot,
    /// 输入、资源、Skill、路由与授权的组合摘要。
    pub context_pack_checksum: String,
    /// 服务端保留、业务服务不解释的字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Agent 快照。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentSnapshot {
    /// Agent UUID。
    pub agent_id: String,
    /// Agent 内部名。
    pub name: String,
    /// Agent 展示名。
    pub display_name: String,
    /// descriptor 摘要。
    pub descriptor_checksum: String,
    /// 绑定的 Runtime。
    pub runtime: RuntimeSnapshot,
    /// 服务端保留、业务服务不解释的字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Runtime 快照。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RuntimeSnapshot {
    /// Runtime UUID。
    pub runtime_id: String,
    /// Runtime 内部名。
    pub name: String,
    /// 来源（builtin / external）。
    pub origin: String,
    /// 适配器类型。
    pub adapter_kind: String,
    /// 服务端保留、业务服务不解释的字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 输入快照。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InputSnapshot {
    /// 用户输入正文。
    pub content: String,
    /// 服务端保留、业务服务不解释的字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 创建/重放 Run 的响应。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RunCreated {
    /// Run 记录。
    pub run: AgentRun,
    /// 是否本次新建。
    pub created: bool,
    /// 服务端保留、业务服务不解释的字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 取消 Run 的响应。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RunCancelled {
    /// 取消后的 Run。
    pub run: AgentRun,
    /// 是否立即进入 cancelled 终态。
    pub cancelled_immediately: bool,
    /// 服务端保留、业务服务不解释的字段。
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// 统一错误响应。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ErrorEnvelope {
    /// 错误体。
    pub error: ErrorBody,
}

/// 错误码与展示说明。
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ErrorBody {
    /// 稳定错误码。
    pub code: String,
    /// 展示说明。
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_run() -> Value {
        serde_json::json!({
            "run_id": "6f5d0f0e-7f4b-4a0e-9d1b-3f5a2c8e9d10",
            "organization_id": "0b6a0c2e-1f3d-4e5a-8b7c-9d0e1f2a3b4c",
            "session_id": "1c7b1d3f-2a4e-4f6b-9c8d-0e1f2a3b4c5d",
            "owner_user_id": 42,
            "agent_id": "2d8c2e40-3b5f-4a7c-8d9e-1f2a3b4c5d6e",
            "runtime_connection_id": "3e9d3f51-4c6a-4b8d-9e0f-2a3b4c5d6e7f",
            "origin": "direct",
            "status": "succeeded",
            "idempotency_key": "sample-key",
            "request_digest": "sha256:abc",
            "parent_run_id": null,
            "delegation_depth": 0,
            "last_event_seq": 3,
            "trace_delivered_through_seq": 3,
            "error_code": null,
            "created_at": "2026-11-04T00:00:00Z",
            "started_at": "2026-11-04T00:00:01Z",
            "finished_at": "2026-11-04T00:00:02Z",
            "snapshot": {
                "version": 1,
                "agent": {
                    "agent_id": "2d8c2e40-3b5f-4a7c-8d9e-1f2a3b4c5d6e",
                    "name": "sample-agent",
                    "display_name": "Sample Agent",
                    "descriptor_checksum": "sha256:def",
                    "runtime": {
                        "runtime_id": "3e9d3f51-4c6a-4b8d-9e0f-2a3b4c5d6e7f",
                        "name": "sample-runtime",
                        "origin": "external",
                        "adapter_kind": "a2a",
                        "probe_detail": "Ready"
                    }
                },
                "input": {
                    "message_id": "4f0e4a62-5d7b-4c9e-8f1a-3b4c5d6e7f80",
                    "content": "hello",
                    "content_checksum": "sha256:ghi"
                },
                "context_pack_checksum": "sha256:jkl",
                "access_policy_checksum": "sha256:mno"
            }
        })
    }

    #[test]
    fn parses_run_created_and_preserves_server_fields() {
        let payload = serde_json::json!({
            "run": fixture_run(),
            "created": true
        });
        let parsed: RunCreated = serde_json::from_value(payload).expect("创建响应应可解析");
        assert!(parsed.created);
        assert_eq!(parsed.run.status, "succeeded");
        assert_eq!(parsed.run.snapshot.agent.runtime.adapter_kind, "a2a");
        assert_eq!(parsed.run.snapshot.input.content, "hello");
        assert_eq!(
            parsed.run.extra.get("parent_run_id"),
            Some(&Value::Null),
            "未声明字段应原样保留"
        );
        assert_eq!(
            parsed.run.snapshot.extra.get("version"),
            Some(&Value::from(1)),
            "快照扩展字段应原样保留"
        );

        let round_trip = serde_json::to_value(parsed).expect("序列化应成功");
        assert_eq!(round_trip["run"]["delegation_depth"], 0);
        assert_eq!(
            round_trip["run"]["snapshot"]["agent"]["runtime"]["probe_detail"],
            "Ready"
        );
    }

    #[test]
    fn parses_cancelled_run_response() {
        let payload = serde_json::json!({
            "run": fixture_run(),
            "cancelled_immediately": false
        });
        let parsed: RunCancelled = serde_json::from_value(payload).expect("取消响应应可解析");
        assert!(!parsed.cancelled_immediately);
        assert_eq!(parsed.run.run_id, "6f5d0f0e-7f4b-4a0e-9d1b-3f5a2c8e9d10");
    }

    #[test]
    fn parses_stable_error_envelope() {
        let parsed: ErrorEnvelope =
            serde_json::from_str(r#"{"error":{"code":"run_not_found","message":"目标不存在"}}"#)
                .expect("错误响应应可解析");
        assert_eq!(parsed.error.code, "run_not_found");
        assert_eq!(parsed.error.message, "目标不存在");
    }

    #[test]
    fn rejects_response_missing_required_field() {
        let mut payload = serde_json::json!({
            "run": fixture_run(),
            "created": true
        });
        payload["run"]
            .as_object_mut()
            .expect("run 对象")
            .remove("run_id");
        let parsed: Result<RunCreated, _> = serde_json::from_value(payload);
        assert!(parsed.is_err(), "缺少 run_id 必须被契约拒绝");
    }
}
