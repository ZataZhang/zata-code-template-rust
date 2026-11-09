//! MCP 网关用例：按调用方授权聚合工具，并在调用时重新校验授权。

use futures_util::{StreamExt, stream};
use serde_json::Value;

use super::McpService;
use crate::connection::Connection;
use crate::error::McpError;
use crate::identity::IdentityContext;
use crate::ports::{RemoteMcpError, RepositoryError};
use crate::tool::{AuthorizedTool, ToolCallOutcome, exposed_tool_name, parse_exposed_tool_name};

impl McpService {
    /// 列出当前主体获准使用的全部工具；最多并发查询 8 个连接，单个失败只跳过该连接。
    pub async fn list_authorized_tools(
        &self,
        identity: &IdentityContext,
    ) -> Result<Vec<AuthorizedTool>, McpError> {
        let scope = identity.scope();
        let subjects = identity.assignment_subjects();
        let connections = self
            .dependencies
            .repository
            .list_authorized(&scope, &subjects)
            .await
            .map_err(Self::map_repository_error)?;

        let mut authorized_tools = Vec::new();
        // 有界并发避免单个慢上游阻塞后续连接，同时限制出站资源占用。
        let mut pending = stream::iter(connections.into_iter().map(|connection| async move {
            (
                connection.id,
                self.collect_connection_tools(&connection).await,
            )
        }))
        .buffer_unordered(8);
        while let Some((connection_id, discovered)) = pending.next().await {
            match discovered {
                Ok(mut connection_tools) => authorized_tools.append(&mut connection_tools),
                Err(error) => {
                    tracing::warn!(
                        connection_id = %connection_id,
                        error = %error,
                        "跳过不可用的 MCP 连接"
                    );
                }
            }
        }
        authorized_tools.sort_by(|left, right| left.exposed_name.cmp(&right.exposed_name));
        Ok(authorized_tools)
    }

    /// 调用某个对外工具名；调用阶段重新查询授权，不复用列表结果。
    pub async fn call_authorized_tool(
        &self,
        identity: &IdentityContext,
        exposed_name: &str,
        arguments: Option<Value>,
    ) -> Result<ToolCallOutcome, McpError> {
        let (connection_id, remote_name) =
            parse_exposed_tool_name(exposed_name).ok_or(McpError::NotFound)?;
        let scope = identity.scope();
        let subjects = identity.assignment_subjects();
        let authorized = self
            .dependencies
            .repository
            .is_authorized(&scope, connection_id, &subjects)
            .await
            .map_err(Self::map_repository_error)?;
        if !authorized {
            return Err(McpError::Forbidden);
        }

        let connection = self
            .dependencies
            .repository
            .find(&scope, connection_id)
            .await
            .map_err(|error| match error {
                RepositoryError::NotFound => McpError::Forbidden,
                other => Self::map_repository_error(other),
            })?;
        let target = self.build_target(&connection).await?;
        self.dependencies
            .remote_client
            .call_tool(&target, &remote_name, arguments)
            .await
            .map_err(|error| match error {
                RemoteMcpError::ToolNotFound => McpError::NotFound,
                other => {
                    tracing::warn!(
                        connection_id = %connection_id,
                        error = %other,
                        "远端工具调用失败"
                    );
                    McpError::UpstreamUnavailable
                }
            })
    }

    async fn collect_connection_tools(
        &self,
        connection: &Connection,
    ) -> Result<Vec<AuthorizedTool>, McpError> {
        if !connection.is_active() {
            return Ok(Vec::new());
        }
        let target = self.build_target(connection).await?;
        let definitions = self
            .dependencies
            .remote_client
            .list_tools(&target)
            .await
            .map_err(|error| {
                tracing::warn!(
                    connection_id = %connection.id,
                    error = %error,
                    "远端工具发现失败"
                );
                McpError::UpstreamUnavailable
            })?;

        let max_tools = self.dependencies.config.max_tools_per_connection;
        if definitions.len() > max_tools {
            tracing::warn!(
                connection_id = %connection.id,
                discovered = definitions.len(),
                limit = max_tools,
                "连接工具数量超过上限，已截断"
            );
        }

        let mut tools = Vec::new();
        for definition in definitions.into_iter().take(max_tools) {
            if definition.name.is_empty() {
                continue;
            }
            let exposed_name = exposed_tool_name(connection.id, &definition.name);
            let mut tool_definition = definition.definition;
            let Some(object) = tool_definition.as_object_mut() else {
                tracing::warn!(
                    connection_id = %connection.id,
                    "远端工具定义不是 JSON 对象，已跳过"
                );
                continue;
            };
            object.insert("name".to_owned(), Value::String(exposed_name.clone()));
            tools.push(AuthorizedTool {
                connection_id: connection.id,
                exposed_name,
                definition: tool_definition,
            });
        }
        Ok(tools)
    }
}
