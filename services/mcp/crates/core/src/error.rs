//! 领域错误：稳定分类，不携带凭据、工具参数或上游内部细节。

/// MCP 服务领域错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum McpError {
    /// 请求字段不合法。
    #[error("请求参数不合法：{0}")]
    InvalidRequest(String),
    /// 调用方服务身份缺失或无效。
    #[error("调用方服务身份无效")]
    Unauthorized,
    /// 调用方无权执行该操作，或工具不在当前主体授权范围内。
    #[error("调用方无权执行该操作")]
    Forbidden,
    /// 连接不存在于当前消费服务与租户范围内。
    #[error("连接不存在")]
    NotFound,
    /// 同一租户内连接名称重复。
    #[error("连接名称已存在")]
    Conflict,
    /// 目标地址不符合出站安全策略。
    #[error("目标地址不符合出站安全策略：{0}")]
    UnsafeEndpoint(String),
    /// 凭据密文无法解密或与认证方式不匹配。
    #[error("连接凭据不可用")]
    CredentialUnavailable,
    /// 远端 MCP server 不可用或返回协议错误。
    #[error("远端 MCP server 不可用")]
    UpstreamUnavailable,
    /// 服务内部错误。
    #[error("服务内部错误")]
    Internal,
}
