//! 连接管理与 MCP 网关用例。

mod gateway;
mod management;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use url::Url;

use crate::connection::{AuthType, Connection, TenantScope};
use crate::error::McpError;
use crate::identity::IdentityContext;
use crate::ports::{
    ConnectionRepository, CredentialCipher, EndpointPolicy, EndpointPolicyError, RemoteMcpClient,
    RemoteTarget, RepositoryError,
};

/// endpoint URL 原始长度上限，与数据库 CHECK 约束保持一致。
const MAX_ENDPOINT_URL_CHARS: usize = 2048;

/// 服务级限制。
#[derive(Debug, Clone)]
pub struct McpServiceConfig {
    /// 单个连接最多暴露的工具数量。
    pub max_tools_per_connection: usize,
}

impl Default for McpServiceConfig {
    fn default() -> Self {
        Self {
            max_tools_per_connection: 256,
        }
    }
}

/// 用例依赖。
pub struct McpServiceDependencies {
    /// 连接与授权仓储。
    pub repository: Arc<dyn ConnectionRepository>,
    /// 凭据认证加密。
    pub cipher: Arc<dyn CredentialCipher>,
    /// 出站地址策略。
    pub endpoint_policy: Arc<dyn EndpointPolicy>,
    /// 远端 MCP Streamable HTTP 客户端。
    pub remote_client: Arc<dyn RemoteMcpClient>,
    /// 服务级限制。
    pub config: McpServiceConfig,
}

/// MCP 服务用例入口；管理与网关共享同一组依赖。
pub struct McpService {
    dependencies: McpServiceDependencies,
}

impl McpService {
    /// 使用给定依赖构造服务。
    pub fn new(dependencies: McpServiceDependencies) -> Self {
        Self { dependencies }
    }

    /// 管理接口只允许带管理角色的签名调用方。
    fn require_admin(identity: &IdentityContext) -> Result<(), McpError> {
        if identity.is_admin() {
            Ok(())
        } else {
            Err(McpError::Forbidden)
        }
    }

    /// 校验并规范化连接名称。
    fn validate_connection_name(name: &str) -> Result<String, McpError> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(McpError::InvalidRequest("连接名称不能为空".to_owned()));
        }
        if trimmed.chars().count() > 100 {
            return Err(McpError::InvalidRequest(
                "连接名称不能超过 100 个字符".to_owned(),
            ));
        }
        if trimmed.chars().any(char::is_control) {
            return Err(McpError::InvalidRequest(
                "连接名称不能包含控制字符".to_owned(),
            ));
        }
        Ok(trimmed.to_owned())
    }

    /// 在出站策略与数据库之前拒绝超长 endpoint，避免落到内部错误。
    fn validate_endpoint_url(endpoint: &Url) -> Result<(), McpError> {
        if endpoint.as_str().chars().count() > MAX_ENDPOINT_URL_CHARS {
            return Err(McpError::InvalidRequest(
                "endpoint_url 不能超过 2048 个字符".to_owned(),
            ));
        }
        Ok(())
    }

    /// 把仓储错误收敛为领域错误；日志只记录安全的外部描述。
    fn map_repository_error(error: RepositoryError) -> McpError {
        match error {
            RepositoryError::NotFound => McpError::NotFound,
            RepositoryError::Conflict => McpError::Conflict,
            RepositoryError::Unavailable(source) | RepositoryError::Other(source) => {
                tracing::warn!(error = %source, "MCP 仓储操作失败");
                McpError::Internal
            }
        }
    }

    /// 把出站策略错误收敛为领域错误。
    fn map_endpoint_error(error: EndpointPolicyError) -> McpError {
        McpError::UnsafeEndpoint(error.to_string())
    }

    /// 重新校验 endpoint、解密凭据并构造出站目标。
    async fn build_target(&self, connection: &Connection) -> Result<RemoteTarget, McpError> {
        let validated = self
            .dependencies
            .endpoint_policy
            .validate(&connection.endpoint_url)
            .await
            .map_err(Self::map_endpoint_error)?;
        let credential = match connection.auth_type {
            AuthType::None => None,
            AuthType::Bearer => Some(
                self.dependencies
                    .cipher
                    .decrypt(&connection.credential, &connection.credential_scope())
                    .map_err(|error| {
                        tracing::warn!(
                            connection_id = %connection.id,
                            error = %error,
                            "连接凭据无法解密"
                        );
                        McpError::CredentialUnavailable
                    })?,
            ),
        };
        Ok(RemoteTarget {
            connection_id: connection.id,
            endpoint_url: validated.url,
            addresses: validated.addresses,
            credential,
        })
    }

    /// 当前调用方的消费服务与租户范围。
    fn scope(identity: &IdentityContext) -> TenantScope {
        identity.scope()
    }
}
