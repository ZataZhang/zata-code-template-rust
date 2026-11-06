//! 稳定的服务错误类别；API 层据此映射 HTTP 状态与错误码。

/// Agent 服务对外可分类的错误。
///
/// 错误消息按中文展示，但调用方应依赖 [`AgentError::code`] 的稳定错误码。
/// 错误里不放凭据、连接串、远端响应正文或完整提示词。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AgentError {
    /// 请求参数或状态转换不合法。
    #[error("请求无效：{0}")]
    InvalidRequest(String),
    /// 缺少或无法识别调用服务凭据。
    #[error("服务凭据无效")]
    ServiceUnauthorized,
    /// 缺少或无法识别最终用户身份。
    #[error("需要有效登录")]
    Unauthenticated,
    /// accounts 身份服务暂时不可达。
    #[error("身份服务暂时不可用")]
    IdentityUnavailable,
    /// 当前身份无权执行该操作；内部代码为稳定拒绝原因。
    #[error("无权执行该操作")]
    Forbidden(&'static str),
    /// 目标不存在或不属于当前调用者；避免跨 owner 泄露存在性。
    #[error("目标不存在")]
    NotFound(&'static str),
    /// 目标状态与请求冲突。
    #[error("目标状态冲突")]
    Conflict(&'static str),
    /// 同一幂等键被用于不同的规范化请求。
    #[error("幂等键已用于不同请求")]
    IdempotencyConflict,
    /// Agent 已绑定 Runtime，普通更新不能改绑。
    #[error("Agent 创建后不能改绑 Runtime")]
    RuntimeBindingImmutable,
    /// Runtime 未登记、未声明或最近发现不可用。
    #[error("Runtime 当前不可用")]
    RuntimeUnavailable,
    /// Agent 被停用或当前配置不可运行。
    #[error("Agent 当前不可运行")]
    AgentNotRunnable,
    /// Auto 入口没有可用候选。
    #[error("没有可运行的 Agent")]
    NoRunnableAgent,
    /// 运行已被取消，后续工作必须停止。
    #[error("Run 已请求取消")]
    RunCancelled,
    /// 请求体或上传超过配置上限。
    #[error("请求体超过上限")]
    PayloadTooLarge,
    /// Content-Type 不受支持。
    #[error("不支持的媒体类型")]
    UnsupportedMediaType,
    /// 执行适配器返回的明确失败类别；错误码由适配器给出。
    #[error("Run 执行失败")]
    Execution(&'static str),
    /// 外部 Runtime 返回了明确失败类别；错误码由适配器给出。
    #[error("外部 Runtime 执行失败")]
    Upstream(&'static str),
    /// 外部 Runtime 或身份服务返回了不可用结果。
    #[error("上游服务暂时不可用")]
    UpstreamUnavailable,
    /// 本地持久化或 Blob 操作失败。
    #[error("存储暂时不可用")]
    StorageUnavailable,
    /// 内部契约被破坏（例如事件投影缺字段），不应暴露实现细节。
    #[error("内部错误")]
    Internal,
}

impl AgentError {
    /// 稳定的机器可读错误码。
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidRequest(_) => "invalid_request",
            Self::ServiceUnauthorized => "service_unauthorized",
            Self::Unauthenticated => "unauthenticated",
            Self::IdentityUnavailable => "identity_unavailable",
            Self::Forbidden(code) => code,
            Self::NotFound(code) => code,
            Self::Conflict(code) => code,
            Self::IdempotencyConflict => "idempotency_conflict",
            Self::RuntimeBindingImmutable => "runtime_binding_immutable",
            Self::RuntimeUnavailable => "runtime_unavailable",
            Self::AgentNotRunnable => "agent_not_runnable",
            Self::NoRunnableAgent => "no_runnable_agent",
            Self::RunCancelled => "run_cancelled",
            Self::PayloadTooLarge => "payload_too_large",
            Self::UnsupportedMediaType => "unsupported_media_type",
            Self::Execution(code) => code,
            Self::Upstream(code) => code,
            Self::UpstreamUnavailable => "upstream_unavailable",
            Self::StorageUnavailable => "storage_unavailable",
            Self::Internal => "internal_error",
        }
    }
}
