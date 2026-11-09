//! Streamable HTTP MCP 网关：按请求身份聚合授权工具，调用阶段重新校验授权。
//!
//! 工具名带稳定连接前缀；单个上游失败只影响该连接。授权错误使用
//! 实现保留范围内的 JSON-RPC 错误码，并在 `data.status` 中携带等价 HTTP 状态。

use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ErrorCode, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::transport::StreamableHttpServerConfig;
use rmcp::transport::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::json;
use tpl_mcp_core::{IdentityContext, McpError, McpService};

/// 调用方身份缺失；正常由 HTTP 中间件拦截，此码用于防御性返回。
const CODE_UNAUTHORIZED: i32 = -32001;
/// 工具未授权给当前主体；语义等同 HTTP 403。
pub const CODE_FORBIDDEN: i32 = -32003;
/// 远端 MCP server 不可用或出站策略拒绝。
const CODE_UPSTREAM_UNAVAILABLE: i32 = -32005;

/// MCP endpoint 的 HTTP 层限制。
#[derive(Debug, Clone)]
pub struct GatewayHttpConfig {
    /// 允许的 Host 头；空列表表示不校验（不推荐）。
    pub allowed_hosts: Vec<String>,
    /// 单个 MCP 请求体上限（字节）。
    pub max_request_body_bytes: usize,
}

/// 网关 handler；调用方身份从入口中间件注入的 HTTP Parts 读取。
#[derive(Clone)]
pub struct GatewayHandler {
    service: Arc<McpService>,
}

impl ServerHandler for GatewayHandler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "tpl-mcp-gateway",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions("按调用方租户与显式 user/group 授权过滤的 MCP 工具网关。")
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let identity = caller_identity(&context)?;
        let tools = self
            .service
            .list_authorized_tools(&identity)
            .await
            .map_err(to_error_data)?;
        let mut responses = Vec::with_capacity(tools.len());
        for tool in tools {
            match serde_json::from_value::<Tool>(tool.definition) {
                Ok(parsed) => responses.push(parsed),
                Err(error) => tracing::warn!(
                    connection_id = %tool.connection_id,
                    error = %error,
                    "远端工具定义无法转换为 MCP Tool，已跳过"
                ),
            }
        }
        Ok(ListToolsResult::with_all_items(responses))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let identity = caller_identity(&context)?;
        let arguments = request.arguments.map(serde_json::Value::Object);
        let outcome = self
            .service
            .call_authorized_tool(&identity, request.name.as_ref(), arguments)
            .await
            .map_err(to_error_data)?;
        let result = serde_json::from_value::<CallToolResult>(outcome.result).map_err(|error| {
            tracing::warn!(error = %error, "远端工具结果无法解析为 MCP 结果");
            ErrorData::internal_error("远端工具结果无法解析", None)
        })?;
        Ok(CallToolResponse::Complete(result))
    }
}

/// 构造 Streamable HTTP MCP 服务；会话关闭、Origin/Host 校验由 rmcp 承担。
pub fn build_streamable_service(
    service: Arc<McpService>,
    config: GatewayHttpConfig,
) -> StreamableHttpService<GatewayHandler, LocalSessionManager> {
    let factory = move || {
        Ok::<GatewayHandler, std::io::Error>(GatewayHandler {
            service: service.clone(),
        })
    };
    let mut server_config = StreamableHttpServerConfig::default();
    server_config.legacy_session_mode = false;
    server_config.json_response = true;
    server_config.allowed_hosts = config.allowed_hosts;
    server_config.max_request_body_bytes = config.max_request_body_bytes;
    let server_config = server_config.enforce_origin_validation();
    StreamableHttpService::new(
        factory,
        Arc::new(LocalSessionManager::default()),
        server_config,
    )
}

fn caller_identity(context: &RequestContext<RoleServer>) -> Result<IdentityContext, ErrorData> {
    context
        .extensions
        .get::<axum::http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<IdentityContext>())
        .cloned()
        .ok_or_else(|| {
            ErrorData::new(
                ErrorCode(CODE_UNAUTHORIZED),
                "调用方身份缺失",
                Some(json!({ "status": 401 })),
            )
        })
}

fn to_error_data(error: McpError) -> ErrorData {
    match error {
        McpError::InvalidRequest(message) => ErrorData::invalid_params(message, None),
        McpError::Unauthorized => ErrorData::new(
            ErrorCode(CODE_UNAUTHORIZED),
            "调用方服务身份无效",
            Some(json!({ "status": 401 })),
        ),
        McpError::Forbidden => ErrorData::new(
            ErrorCode(CODE_FORBIDDEN),
            "调用方无权使用该工具",
            Some(json!({ "status": 403 })),
        ),
        McpError::NotFound => {
            ErrorData::resource_not_found("工具不存在", Some(json!({ "status": 404 })))
        }
        McpError::Conflict => ErrorData::internal_error("工具调用状态冲突", None),
        McpError::UnsafeEndpoint(_)
        | McpError::CredentialUnavailable
        | McpError::UpstreamUnavailable => ErrorData::new(
            ErrorCode(CODE_UPSTREAM_UNAVAILABLE),
            "远端 MCP server 不可用",
            Some(json!({ "status": 502 })),
        ),
        McpError::Internal => ErrorData::internal_error("服务内部错误", None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error_status(data: &ErrorData) -> Option<i64> {
        data.data
            .as_ref()
            .and_then(|value| value.get("status"))
            .and_then(serde_json::Value::as_i64)
    }

    #[test]
    fn authorization_errors_use_documented_json_rpc_codes() {
        let forbidden = to_error_data(McpError::Forbidden);
        assert_eq!(forbidden.code, ErrorCode(CODE_FORBIDDEN));
        assert_eq!(error_status(&forbidden), Some(403));

        let unauthorized = to_error_data(McpError::Unauthorized);
        assert_eq!(unauthorized.code, ErrorCode(CODE_UNAUTHORIZED));
        assert_eq!(error_status(&unauthorized), Some(401));

        let not_found = to_error_data(McpError::NotFound);
        assert_eq!(not_found.code, ErrorCode::RESOURCE_NOT_FOUND);
        assert_eq!(error_status(&not_found), Some(404));
    }

    #[test]
    fn upstream_failures_are_reported_as_unavailable_without_remote_details() {
        let unavailable = to_error_data(McpError::UpstreamUnavailable);
        assert_eq!(unavailable.code, ErrorCode(CODE_UPSTREAM_UNAVAILABLE));
        assert_eq!(error_status(&unavailable), Some(502));
        assert!(!unavailable.message.contains("http"));

        let unsafe_endpoint = to_error_data(McpError::UnsafeEndpoint("10.0.0.1".to_owned()));
        assert_eq!(unsafe_endpoint.code, ErrorCode(CODE_UPSTREAM_UNAVAILABLE));
        assert!(!unsafe_endpoint.message.contains("10.0.0.1"));

        let invalid = to_error_data(McpError::InvalidRequest("参数错误".to_owned()));
        assert_eq!(invalid.code, ErrorCode::INVALID_PARAMS);
    }
}
