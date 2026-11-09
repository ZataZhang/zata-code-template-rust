//! 基于 rmcp 的远端 MCP Streamable HTTP 客户端。

use std::future::Future;
use std::time::Duration;

use async_trait::async_trait;
use rmcp::model::{CallToolRequestParams, ErrorCode, PaginatedRequestParams};
use rmcp::service::RunningService;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use rmcp::{RoleClient, ServiceError, ServiceExt};
use serde_json::Value;
use tpl_mcp_core::{
    RemoteMcpClient, RemoteMcpError, RemoteTarget, RemoteToolDefinition, ToolCallOutcome,
};
use url::{Host, Url};

use crate::bounded_http_client::BoundedReqwestClient;
use crate::config::OutboundSettings;

/// 关闭远端会话等待清理的时间上限。
const SESSION_CLOSE_TIMEOUT: Duration = Duration::from_millis(500);

/// 使用 rmcp 官方 Streamable HTTP client 的远端调用实现。
pub struct RmcpRemoteClient {
    connect_timeout: Duration,
    request_timeout: Duration,
    max_response_bytes: usize,
}

impl RmcpRemoteClient {
    /// 从出站配置创建客户端。
    pub fn new(settings: &OutboundSettings) -> Self {
        Self {
            connect_timeout: Duration::from_millis(settings.connect_timeout_ms.clamp(100, 60_000)),
            request_timeout: Duration::from_millis(settings.request_timeout_ms.clamp(100, 120_000)),
            max_response_bytes: settings.max_response_bytes.max(1024),
        }
    }

    fn http_client(&self, target: &RemoteTarget) -> Result<reqwest::Client, RemoteMcpError> {
        let mut builder = reqwest::Client::builder()
            // 禁用重定向，避免安全校验后的跳转绕过地址策略。
            .redirect(reqwest::redirect::Policy::none())
            // 不读取系统代理环境变量，代理会绕过已校验的目标地址。
            .no_proxy()
            .connect_timeout(self.connect_timeout)
            .timeout(self.request_timeout);
        if let Some(domain) = pinnable_domain(&target.endpoint_url) {
            // 固定已校验的解析结果，防止 DNS rebinding。
            builder = builder.resolve_to_addrs(&domain, &target.addresses);
        }
        builder.build().map_err(|_| RemoteMcpError::Unavailable)
    }

    async fn connect(
        &self,
        target: &RemoteTarget,
    ) -> Result<RunningService<RoleClient, ()>, RemoteMcpError> {
        let client = self.http_client(target)?;
        let mut config =
            StreamableHttpClientTransportConfig::with_uri(target.endpoint_url.as_str())
                .max_sse_event_size(self.max_response_bytes)
                .max_concurrent_requests(1)
                .control_request_timeout(self.request_timeout.min(Duration::from_secs(5)))
                // 每次调用都新建会话，不需要自动恢复过期会话。
                .reinit_on_expired_session(false);
        if let Some(credential) = &target.credential {
            config = config.auth_header(credential.expose().to_owned());
        }
        let transport = StreamableHttpClientTransport::with_client(
            BoundedReqwestClient::new(client, self.max_response_bytes),
            config,
        );
        match tokio::time::timeout(self.request_timeout, ().serve(transport)).await {
            Ok(Ok(service)) => Ok(service),
            Ok(Err(_error)) => {
                tracing::debug!(
                    connection_id = %target.connection_id,
                    "远端 MCP server 初始化握手失败"
                );
                Err(RemoteMcpError::Unavailable)
            }
            Err(_elapsed) => {
                tracing::debug!(
                    connection_id = %target.connection_id,
                    "远端 MCP server 初始化握手超时"
                );
                Err(RemoteMcpError::Unavailable)
            }
        }
    }

    async fn with_session<T, F, Fut>(
        &self,
        target: &RemoteTarget,
        operation: F,
    ) -> Result<T, RemoteMcpError>
    where
        F: FnOnce(RunningService<RoleClient, ()>) -> Fut,
        Fut: Future<Output = Result<T, RemoteMcpError>> + Send,
    {
        let service = self.connect(target).await?;
        match tokio::time::timeout(self.request_timeout, operation(service)).await {
            Ok(result) => result,
            Err(_elapsed) => {
                tracing::debug!(
                    connection_id = %target.connection_id,
                    "远端 MCP 调用超出整体超时"
                );
                Err(RemoteMcpError::Unavailable)
            }
        }
    }

    fn encoded_size(&self, value: &Value) -> Result<usize, RemoteMcpError> {
        serde_json::to_vec(value)
            .map(|encoded| encoded.len())
            .map_err(|_| RemoteMcpError::Protocol)
    }
}

#[async_trait]
impl RemoteMcpClient for RmcpRemoteClient {
    async fn list_tools(
        &self,
        target: &RemoteTarget,
    ) -> Result<Vec<RemoteToolDefinition>, RemoteMcpError> {
        self.with_session(target, |mut service| async move {
            let discovery_result = async {
                let mut next_cursor = None;
                let mut total_bytes = 0usize;
                let mut definitions = Vec::new();

                loop {
                    let page = service
                        .list_tools(Some(
                            PaginatedRequestParams::default().with_cursor(next_cursor),
                        ))
                        .await
                        .map_err(map_discovery_error)?;
                    next_cursor = page.next_cursor;

                    for tool in page.tools {
                        // 在把工具追加到跨页结果前检查累计值，避免 list_all_tools
                        // 先保留所有分页后才按配置上限拒绝。
                        let encoded_tool =
                            serde_json::to_vec(&tool).map_err(|_| RemoteMcpError::Protocol)?;
                        total_bytes = total_bytes.saturating_add(encoded_tool.len());
                        if total_bytes > self.max_response_bytes {
                            tracing::debug!(
                                connection_id = %target.connection_id,
                                "远端 tools/list 结果超出出站大小上限"
                            );
                            return Err(RemoteMcpError::Protocol);
                        }
                        drop(encoded_tool);

                        let definition =
                            serde_json::to_value(&tool).map_err(|_| RemoteMcpError::Protocol)?;
                        definitions.push(RemoteToolDefinition {
                            name: tool.name.to_string(),
                            definition,
                        });
                    }

                    if next_cursor.is_none() {
                        break;
                    }
                }

                Ok(definitions)
            }
            .await;
            close_session(&mut service).await;
            discovery_result
        })
        .await
    }

    async fn call_tool(
        &self,
        target: &RemoteTarget,
        tool_name: &str,
        arguments: Option<Value>,
    ) -> Result<ToolCallOutcome, RemoteMcpError> {
        let params = match arguments {
            None => CallToolRequestParams::new(tool_name.to_owned()),
            Some(value) => {
                let object = value.as_object().cloned().ok_or(RemoteMcpError::Protocol)?;
                CallToolRequestParams::new(tool_name.to_owned()).with_arguments(object)
            }
        };
        let result = self
            .with_session(target, |mut service| async move {
                let called = service.call_tool(params).await.map_err(map_call_error)?;
                close_session(&mut service).await;
                Ok(called)
            })
            .await?;

        let payload = serde_json::to_value(&result).map_err(|_| RemoteMcpError::Protocol)?;
        if self.encoded_size(&payload)? > self.max_response_bytes {
            tracing::debug!(
                connection_id = %target.connection_id,
                "远端 tools/call 结果超出出站大小上限"
            );
            return Err(RemoteMcpError::Protocol);
        }
        Ok(ToolCallOutcome {
            is_error: result.is_error.unwrap_or(false),
            result: payload,
        })
    }
}

/// 仅当 endpoint 使用域名时才需要固定解析结果；IP 字面量不经过 DNS。
fn pinnable_domain(endpoint: &Url) -> Option<String> {
    match endpoint.host()? {
        Host::Domain(domain) => Some(domain.to_owned()),
        Host::Ipv4(_) | Host::Ipv6(_) => None,
    }
}

async fn close_session(service: &mut RunningService<RoleClient, ()>) {
    if service
        .close_with_timeout(SESSION_CLOSE_TIMEOUT)
        .await
        .is_err()
    {
        tracing::debug!("远端 MCP 会话关闭时后台任务未正常结束");
    }
}

fn map_discovery_error(error: ServiceError) -> RemoteMcpError {
    match error {
        ServiceError::UnexpectedResponse => RemoteMcpError::Protocol,
        _ => RemoteMcpError::Unavailable,
    }
}

fn map_call_error(error: ServiceError) -> RemoteMcpError {
    match error {
        ServiceError::McpError(data) => {
            if data.code == ErrorCode::METHOD_NOT_FOUND
                || data.code == ErrorCode::RESOURCE_NOT_FOUND
            {
                RemoteMcpError::ToolNotFound
            } else {
                RemoteMcpError::ToolFailed
            }
        }
        ServiceError::UnexpectedResponse => RemoteMcpError::Protocol,
        _ => RemoteMcpError::Unavailable,
    }
}
