//! 基础设施端口：仓储、凭据加密、出站策略、远端 MCP 与就绪探测。

use std::net::SocketAddr;

use async_trait::async_trait;
use url::Url;
use uuid::Uuid;

use crate::connection::{Assignment, AssignmentSubject, Connection, TenantScope};
use crate::secret::{CredentialCiphertext, SecretString};
use crate::tool::{RemoteToolDefinition, ToolCallOutcome};

/// 仓储错误；不向上层暴露 SQL 或连接串细节。
#[derive(Debug, thiserror::Error)]
pub enum RepositoryError {
    /// 目标记录不存在于当前消费服务与租户范围。
    #[error("记录不存在")]
    NotFound,
    /// 唯一约束冲突。
    #[error("记录冲突")]
    Conflict,
    /// 数据库暂时不可用。
    #[error("数据库不可用")]
    Unavailable(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// 其他持久化失败。
    #[error("持久化失败")]
    Other(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// 连接与授权分配仓储；所有查询都必须限定在消费服务与租户范围内。
#[async_trait]
pub trait ConnectionRepository: Send + Sync {
    /// 插入新连接。
    async fn create(&self, connection: &Connection) -> Result<(), RepositoryError>;

    /// 更新连接可变字段。
    async fn update(&self, connection: &Connection) -> Result<(), RepositoryError>;

    /// 删除连接；关联授权一并删除。
    async fn delete(&self, scope: &TenantScope, connection_id: Uuid)
    -> Result<(), RepositoryError>;

    /// 在范围内读取单个连接。
    async fn find(
        &self,
        scope: &TenantScope,
        connection_id: Uuid,
    ) -> Result<Connection, RepositoryError>;

    /// 列出范围内的全部连接。
    async fn list(&self, scope: &TenantScope) -> Result<Vec<Connection>, RepositoryError>;

    /// 列出主体集合获准使用、且处于启用状态的连接。
    async fn list_authorized(
        &self,
        scope: &TenantScope,
        subjects: &[AssignmentSubject],
    ) -> Result<Vec<Connection>, RepositoryError>;

    /// 判断主体集合是否获准使用某个启用中的连接。
    async fn is_authorized(
        &self,
        scope: &TenantScope,
        connection_id: Uuid,
        subjects: &[AssignmentSubject],
    ) -> Result<bool, RepositoryError>;

    /// 列出某连接的授权主体。
    async fn list_assignments(
        &self,
        scope: &TenantScope,
        connection_id: Uuid,
    ) -> Result<Vec<Assignment>, RepositoryError>;

    /// 新增授权；已存在时保持幂等成功。
    async fn add_assignment(
        &self,
        scope: &TenantScope,
        assignment: &Assignment,
    ) -> Result<(), RepositoryError>;

    /// 移除授权；不存在时保持幂等成功。
    async fn remove_assignment(
        &self,
        scope: &TenantScope,
        connection_id: Uuid,
        subject: &AssignmentSubject,
    ) -> Result<(), RepositoryError>;
}

/// 依赖就绪检查失败；只说明依赖名称。
#[derive(Debug, thiserror::Error)]
#[error("{dependency} 未就绪")]
pub struct ReadinessError {
    /// 未就绪的依赖名称。
    pub dependency: &'static str,
}

/// 判断服务能否接收流量。
#[async_trait]
pub trait ReadinessProbe: Send + Sync {
    /// 检查关键依赖。
    async fn check_ready(&self) -> Result<(), ReadinessError>;
}

/// 凭据绑定的身份范围，作为认证加密的附加数据。
#[derive(Debug, Clone, Copy)]
pub struct CredentialScope<'a> {
    /// 消费服务 ID。
    pub consumer_id: &'a str,
    /// 租户 ID。
    pub tenant_id: &'a str,
    /// 连接 ID。
    pub connection_id: Uuid,
}

/// 凭据加解密错误；不携带密钥或明文。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CryptoError {
    /// 加密失败。
    #[error("凭据加密失败")]
    Encrypt,
    /// 解密失败；密文损坏、密钥不匹配或范围被篡改。
    #[error("凭据解密失败")]
    Decrypt,
    /// 配置的主密钥无效。
    #[error("凭据加密配置无效")]
    Configuration,
}

/// 凭据认证加密端口。
pub trait CredentialCipher: Send + Sync {
    /// 使用主密钥和范围附加数据加密明文。
    fn encrypt(
        &self,
        plaintext: &SecretString,
        scope: &CredentialScope<'_>,
    ) -> Result<CredentialCiphertext, CryptoError>;

    /// 使用主密钥和范围附加数据解密密文。
    fn decrypt(
        &self,
        ciphertext: &CredentialCiphertext,
        scope: &CredentialScope<'_>,
    ) -> Result<SecretString, CryptoError>;
}

/// 校验后的出站目标；地址已按策略解析并固定。
#[derive(Debug, Clone)]
pub struct ValidatedEndpoint {
    /// 原始 endpoint。
    pub url: Url,
    /// 已校验的目标地址；出站客户端必须连接这些地址而不是重新解析。
    pub addresses: Vec<SocketAddr>,
}

/// 出站地址策略错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EndpointPolicyError {
    /// 只允许 HTTP 或 HTTPS。
    #[error("仅支持 HTTP 或 HTTPS 地址")]
    UnsupportedScheme,
    /// 非 loopback 目标必须使用 HTTPS。
    #[error("非本机目标必须使用 HTTPS")]
    InsecureScheme,
    /// 缺少主机名或端口。
    #[error("目标地址缺少主机名")]
    MissingHost,
    /// 地址包含不允许的组成部分（用户信息、片段等）。
    #[error("目标地址格式不合法")]
    InvalidUrl,
    /// 命中禁止访问的地址范围。
    #[error("禁止访问本机、私网、链路本地、组播、保留地址或云元数据目标")]
    ForbiddenAddress,
    /// 域名无法解析。
    #[error("目标主机无法解析")]
    ResolutionFailed,
}

/// 保存与消费路径都必须经过的出站地址策略。
#[async_trait]
pub trait EndpointPolicy: Send + Sync {
    /// 解析并校验目标；任何不确定情况都必须失败。
    async fn validate(&self, endpoint: &Url) -> Result<ValidatedEndpoint, EndpointPolicyError>;
}

/// 一次上游调用的目标。
#[derive(Debug, Clone)]
pub struct RemoteTarget {
    /// 连接 ID，仅用于日志关联。
    pub connection_id: Uuid,
    /// 原始 endpoint。
    pub endpoint_url: Url,
    /// 已校验并固定的目标地址。
    pub addresses: Vec<SocketAddr>,
    /// 可选的 Bearer 凭据。
    pub credential: Option<SecretString>,
}

/// 远端 MCP 调用错误；不包含上游响应正文。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RemoteMcpError {
    /// 无法建立连接或完成握手。
    #[error("远端 MCP server 不可用")]
    Unavailable,
    /// 远端返回不符合 MCP 协议的内容。
    #[error("远端 MCP server 返回协议错误")]
    Protocol,
    /// 远端不存在请求的工具。
    #[error("远端工具不存在")]
    ToolNotFound,
    /// 远端工具执行失败。
    #[error("远端工具调用失败")]
    ToolFailed,
}

/// 远端 MCP Streamable HTTP 客户端端口。
#[async_trait]
pub trait RemoteMcpClient: Send + Sync {
    /// 发现远端工具列表。
    async fn list_tools(
        &self,
        target: &RemoteTarget,
    ) -> Result<Vec<RemoteToolDefinition>, RemoteMcpError>;

    /// 调用远端工具并返回序列化结果。
    async fn call_tool(
        &self,
        target: &RemoteTarget,
        tool_name: &str,
        arguments: Option<serde_json::Value>,
    ) -> Result<ToolCallOutcome, RemoteMcpError>;
}
