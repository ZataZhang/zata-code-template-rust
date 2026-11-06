//! Agent 服务执行适配器：内置 Sandbox 平台工具与外部 A2A 1.0 JSON-RPC。

pub mod a2a;
pub mod sandbox;
pub mod skills;

use std::sync::Arc;

use async_trait::async_trait;
use tpl_agent_core::error::AgentError;
use tpl_agent_core::ports::{CredentialResolver, ExecutionOutcome, ExecutionRequest, RunExecutor};
use tpl_agent_core::runtime_catalog::RuntimeAdapterKind;

/// 按 Run 快照中的 Runtime 协议分派到 Sandbox 或 A2A 适配器。
pub struct CompositeRunExecutor {
    sandbox: sandbox::SandboxExecutor,
    a2a: a2a::A2aExecutor,
}

impl CompositeRunExecutor {
    /// 组装两类执行器。
    ///
    /// # Errors
    ///
    /// A2A HTTP 客户端初始化失败时返回内部错误。
    pub fn new(credentials: Arc<dyn CredentialResolver>) -> Result<Self, AgentError> {
        Ok(Self {
            sandbox: sandbox::SandboxExecutor::new(),
            a2a: a2a::A2aExecutor::new(credentials)?,
        })
    }
}

#[async_trait]
impl RunExecutor for CompositeRunExecutor {
    async fn execute(&self, request: ExecutionRequest) -> Result<ExecutionOutcome, AgentError> {
        match request.run.snapshot.runtime().adapter_kind {
            RuntimeAdapterKind::Sandbox => self.sandbox.execute(request).await,
            RuntimeAdapterKind::A2a => self.a2a.execute(request).await,
        }
    }
}
