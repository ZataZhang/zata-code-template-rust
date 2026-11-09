//! MCP Gateway 服务的领域模型、授权规则、连接用例与基础设施端口。

#![forbid(unsafe_code)]

pub mod connection;
pub mod error;
pub mod identity;
pub mod ids;
pub mod ports;
pub mod secret;
pub mod service;
pub mod tool;

pub use connection::{
    Assignment, AssignmentSubject, AuthType, Connection, ConnectionChanges, ConnectionStatus,
    NewConnection, SubjectType, TenantScope,
};
pub use error::McpError;
pub use identity::{AuthError, CallerAuthenticator, CallerRole, IdentityContext};
pub use ids::{ConsumerId, GroupId, SubjectId, TenantId};
pub use ports::{
    ConnectionRepository, CredentialCipher, CredentialScope, CryptoError, EndpointPolicy,
    EndpointPolicyError, ReadinessError, ReadinessProbe, RemoteMcpClient, RemoteMcpError,
    RemoteTarget, RepositoryError, ValidatedEndpoint,
};
pub use secret::{CredentialCiphertext, SecretString};
pub use service::{McpService, McpServiceConfig, McpServiceDependencies};
pub use tool::{
    AuthorizedTool, RemoteToolDefinition, ToolCallOutcome, exposed_tool_name,
    parse_exposed_tool_name,
};
