//! 工具命名与调用结果。
//!
//! 对外工具名由稳定连接前缀与远端原名组成，解析后可还原远端工具名，
//! 因此不同连接的同名工具不会冲突。

use serde_json::Value;
use uuid::Uuid;

const TOOL_PREFIX: &str = "mcp_";

/// 远端 MCP `tools/list` 返回的单个工具（原始 JSON 定义）。
#[derive(Debug, Clone)]
pub struct RemoteToolDefinition {
    /// 远端工具原名。
    pub name: String,
    /// 原样转发的工具定义。
    pub definition: Value,
}

/// 对当前调用主体可用的工具。
#[derive(Debug, Clone)]
pub struct AuthorizedTool {
    /// 所属连接。
    pub connection_id: Uuid,
    /// 提供给 MCP 客户端的稳定名称。
    pub exposed_name: String,
    /// 原始工具定义，`name` 已替换为对外名称。
    pub definition: Value,
}

/// 工具调用结果。
#[derive(Debug, Clone)]
pub struct ToolCallOutcome {
    /// 远端是否把结果标记为工具错误。
    pub is_error: bool,
    /// 序列化后的 MCP `CallToolResult`。
    pub result: Value,
}

/// 生成带连接前缀的稳定工具名。
pub fn exposed_tool_name(connection_id: Uuid, remote_name: &str) -> String {
    format!("{TOOL_PREFIX}{}_{remote_name}", connection_id.simple())
}

/// 解析带连接前缀的工具名，返回连接 ID 与远端原名。
pub fn parse_exposed_tool_name(exposed_name: &str) -> Option<(Uuid, String)> {
    let remainder = exposed_name.strip_prefix(TOOL_PREFIX)?;
    let (connection_hex, remote_name) = remainder.split_once('_')?;
    if remote_name.is_empty() {
        return None;
    }
    let connection_id = Uuid::parse_str(connection_hex).ok()?;
    if connection_id.simple().to_string() != connection_hex {
        return None;
    }
    Some((connection_id, remote_name.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaced_names_round_trip_and_reject_other_shapes() {
        let connection_id = Uuid::from_u128(0x1234_5678_9abc_def0_1234_5678_9abc_def0);
        let exposed = exposed_tool_name(connection_id, "search.docs");
        assert_eq!(
            parse_exposed_tool_name(&exposed),
            Some((connection_id, "search.docs".to_owned()))
        );
        assert_eq!(parse_exposed_tool_name("search.docs"), None);
        assert_eq!(
            parse_exposed_tool_name("mcp_1234_5678_9abc_def0_1234_5678_9abc_def0_docs"),
            None
        );
        assert_eq!(
            parse_exposed_tool_name("mcp_123456789abcdef0123456789abcdef0_"),
            None
        );
    }

    #[test]
    fn same_remote_name_in_different_connections_is_unambiguous() {
        let first = Uuid::from_u128(1);
        let second = Uuid::from_u128(2);
        assert_ne!(
            exposed_tool_name(first, "lookup"),
            exposed_tool_name(second, "lookup")
        );
    }
}
