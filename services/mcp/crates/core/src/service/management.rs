//! 管理用例：连接的增删改查、凭据轮换、检查与授权分配。

use time::OffsetDateTime;
use uuid::Uuid;

use super::McpService;
use crate::connection::{
    Assignment, AssignmentSubject, AuthType, Connection, ConnectionChanges, ConnectionStatus,
    NewConnection,
};
use crate::error::McpError;
use crate::identity::IdentityContext;
use crate::ports::{CredentialScope, RepositoryError};
use crate::secret::{CredentialCiphertext, SecretString};
use crate::tool::RemoteToolDefinition;

/// 凭据明文上限，避免把超大内容写入数据库。
const MAX_CREDENTIAL_BYTES: usize = 4096;

impl McpService {
    /// 创建连接；endpoint 必须先通过出站地址策略。
    pub async fn create_connection(
        &self,
        identity: &IdentityContext,
        input: NewConnection,
    ) -> Result<Connection, McpError> {
        Self::require_admin(identity)?;
        let name = Self::validate_connection_name(&input.name)?;
        let credential = prepare_credential(input.auth_type, input.credential)?;
        Self::validate_endpoint_url(&input.endpoint_url)?;
        let validated = self
            .dependencies
            .endpoint_policy
            .validate(&input.endpoint_url)
            .await
            .map_err(Self::map_endpoint_error)?;

        let connection_id = Uuid::new_v4();
        let scope = Self::scope(identity);
        let credential_ciphertext = self.encrypt_credential(
            connection_id,
            scope.consumer_id.as_str(),
            scope.tenant_id.as_str(),
            credential,
        )?;
        let now = OffsetDateTime::now_utc();
        let connection = Connection {
            id: connection_id,
            scope,
            name,
            endpoint_url: validated.url,
            auth_type: input.auth_type,
            credential: credential_ciphertext,
            status: ConnectionStatus::Active,
            created_by: identity.subject_id.clone(),
            created_at: now,
            updated_at: now,
        };
        self.dependencies
            .repository
            .create(&connection)
            .await
            .map_err(Self::map_repository_error)?;
        Ok(connection)
    }

    /// 更新连接元数据；endpoint 变更时重新校验，切换认证方式时同步凭据状态。
    pub async fn update_connection(
        &self,
        identity: &IdentityContext,
        connection_id: Uuid,
        changes: ConnectionChanges,
    ) -> Result<Connection, McpError> {
        Self::require_admin(identity)?;
        let scope = Self::scope(identity);
        let mut connection = self.load_connection(&scope, connection_id).await?;

        if let Some(name) = changes.name {
            connection.name = Self::validate_connection_name(&name)?;
        }
        if let Some(endpoint_url) = changes.endpoint_url {
            Self::validate_endpoint_url(&endpoint_url)?;
            let validated = self
                .dependencies
                .endpoint_policy
                .validate(&endpoint_url)
                .await
                .map_err(Self::map_endpoint_error)?;
            connection.endpoint_url = validated.url;
        }
        if let Some(status) = changes.status {
            connection.status = status;
        }
        if let Some(auth_type) = changes.auth_type {
            match auth_type {
                AuthType::None => {
                    connection.credential = CredentialCiphertext::none();
                }
                AuthType::Bearer => {
                    if !connection.credential.is_configured() {
                        return Err(McpError::InvalidRequest(
                            "切换到 Bearer 认证前请先设置凭据".to_owned(),
                        ));
                    }
                }
            }
            connection.auth_type = auth_type;
        }
        connection.updated_at = OffsetDateTime::now_utc();
        self.dependencies
            .repository
            .update(&connection)
            .await
            .map_err(Self::map_repository_error)?;
        Ok(connection)
    }

    /// 设置或轮换凭据；响应只返回 `credential_configured`。
    pub async fn set_credential(
        &self,
        identity: &IdentityContext,
        connection_id: Uuid,
        auth_type: AuthType,
        credential: Option<SecretString>,
    ) -> Result<Connection, McpError> {
        Self::require_admin(identity)?;
        let credential = prepare_credential(auth_type, credential)?;
        let scope = Self::scope(identity);
        let mut connection = self.load_connection(&scope, connection_id).await?;
        connection.credential = self.encrypt_credential(
            connection.id,
            scope.consumer_id.as_str(),
            scope.tenant_id.as_str(),
            credential,
        )?;
        connection.auth_type = auth_type;
        connection.updated_at = OffsetDateTime::now_utc();
        self.dependencies
            .repository
            .update(&connection)
            .await
            .map_err(Self::map_repository_error)?;
        Ok(connection)
    }

    /// 停用连接；工具列表与调用会立即忽略它。
    pub async fn disable_connection(
        &self,
        identity: &IdentityContext,
        connection_id: Uuid,
    ) -> Result<Connection, McpError> {
        Self::require_admin(identity)?;
        let scope = Self::scope(identity);
        let mut connection = self.load_connection(&scope, connection_id).await?;
        connection.status = ConnectionStatus::Disabled;
        connection.updated_at = OffsetDateTime::now_utc();
        self.dependencies
            .repository
            .update(&connection)
            .await
            .map_err(Self::map_repository_error)?;
        Ok(connection)
    }

    /// 删除连接及其授权。
    pub async fn delete_connection(
        &self,
        identity: &IdentityContext,
        connection_id: Uuid,
    ) -> Result<(), McpError> {
        Self::require_admin(identity)?;
        let scope = Self::scope(identity);
        self.dependencies
            .repository
            .delete(&scope, connection_id)
            .await
            .map_err(Self::map_repository_error)
    }

    /// 在消费服务与租户范围内读取连接。
    pub async fn get_connection(
        &self,
        identity: &IdentityContext,
        connection_id: Uuid,
    ) -> Result<Connection, McpError> {
        Self::require_admin(identity)?;
        let scope = Self::scope(identity);
        self.load_connection(&scope, connection_id).await
    }

    /// 列出当前租户的全部连接。
    pub async fn list_connections(
        &self,
        identity: &IdentityContext,
    ) -> Result<Vec<Connection>, McpError> {
        Self::require_admin(identity)?;
        let scope = Self::scope(identity);
        self.dependencies
            .repository
            .list(&scope)
            .await
            .map_err(Self::map_repository_error)
    }

    /// 检查连接并发现远端工具；不执行任何业务工具。
    pub async fn check_connection(
        &self,
        identity: &IdentityContext,
        connection_id: Uuid,
    ) -> Result<Vec<RemoteToolDefinition>, McpError> {
        Self::require_admin(identity)?;
        let scope = Self::scope(identity);
        let connection = self.load_connection(&scope, connection_id).await?;
        let target = self.build_target(&connection).await?;
        self.dependencies
            .remote_client
            .list_tools(&target)
            .await
            .map_err(|error| {
                tracing::warn!(
                    connection_id = %connection.id,
                    error = %error,
                    "连接检查失败"
                );
                McpError::UpstreamUnavailable
            })
    }

    /// 列出连接已授予的用户与分组。
    pub async fn list_assignments(
        &self,
        identity: &IdentityContext,
        connection_id: Uuid,
    ) -> Result<Vec<Assignment>, McpError> {
        Self::require_admin(identity)?;
        let scope = Self::scope(identity);
        self.dependencies
            .repository
            .list_assignments(&scope, connection_id)
            .await
            .map_err(Self::map_repository_error)
    }

    /// 新增用户或分组授权；重复授权保持成功。
    pub async fn add_assignment(
        &self,
        identity: &IdentityContext,
        connection_id: Uuid,
        subject: AssignmentSubject,
    ) -> Result<(), McpError> {
        Self::require_admin(identity)?;
        let scope = Self::scope(identity);
        self.dependencies
            .repository
            .add_assignment(
                &scope,
                &Assignment {
                    connection_id,
                    subject,
                },
            )
            .await
            .map_err(Self::map_repository_error)
    }

    /// 移除用户或分组授权；不存在时保持成功。
    pub async fn remove_assignment(
        &self,
        identity: &IdentityContext,
        connection_id: Uuid,
        subject: &AssignmentSubject,
    ) -> Result<(), McpError> {
        Self::require_admin(identity)?;
        let scope = Self::scope(identity);
        self.dependencies
            .repository
            .remove_assignment(&scope, connection_id, subject)
            .await
            .map_err(Self::map_repository_error)
    }

    async fn load_connection(
        &self,
        scope: &crate::connection::TenantScope,
        connection_id: Uuid,
    ) -> Result<Connection, McpError> {
        self.dependencies
            .repository
            .find(scope, connection_id)
            .await
            .map_err(|error| match error {
                RepositoryError::NotFound => McpError::NotFound,
                other => Self::map_repository_error(other),
            })
    }

    fn encrypt_credential(
        &self,
        connection_id: Uuid,
        consumer_id: &str,
        tenant_id: &str,
        credential: Option<SecretString>,
    ) -> Result<CredentialCiphertext, McpError> {
        let Some(plaintext) = credential else {
            return Ok(CredentialCiphertext::none());
        };
        self.dependencies
            .cipher
            .encrypt(
                &plaintext,
                &CredentialScope {
                    consumer_id,
                    tenant_id,
                    connection_id,
                },
            )
            .map_err(|error| {
                tracing::error!(error = %error, "凭据加密失败");
                McpError::Internal
            })
    }
}

/// 校验认证方式与凭据是否匹配；错误信息不包含凭据内容。
fn prepare_credential(
    auth_type: AuthType,
    credential: Option<SecretString>,
) -> Result<Option<SecretString>, McpError> {
    match (auth_type, credential) {
        (AuthType::None, Some(_)) => {
            Err(McpError::InvalidRequest("匿名连接不能提交凭据".to_owned()))
        }
        (AuthType::Bearer, None) => Err(McpError::InvalidRequest(
            "Bearer 连接必须提交凭据".to_owned(),
        )),
        (AuthType::None, None) => Ok(None),
        (AuthType::Bearer, Some(credential)) => {
            let value = credential.expose();
            if value.is_empty() || value.len() > MAX_CREDENTIAL_BYTES {
                return Err(McpError::InvalidRequest("凭据长度不合法".to_owned()));
            }
            if value.chars().any(|character| {
                !character.is_ascii() || character.is_control() || character.is_whitespace()
            }) {
                return Err(McpError::InvalidRequest(
                    "凭据只能包含可见 ASCII 字符".to_owned(),
                ));
            }
            Ok(Some(credential))
        }
    }
}
