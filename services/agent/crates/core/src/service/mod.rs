//! 用例编排：依赖注入、启动对齐、审计与共享授权辅助。

pub mod admin;
pub mod execution;
pub mod public;

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::Value;
use uuid::Uuid;

use crate::agent_catalog::{AgentAccessRule, BusinessAgent};
use crate::audit::{AuditOutcome, NewAuditRecord};
use crate::delegation::{DelegationChildRef, DelegationSnapshot, delegation_snapshot};
use crate::error::AgentError;
use crate::identity::{AdminIdentity, AdminIdentityValidator, UserIdentityValidator};
use crate::organization::{OrganizationMembership, PlatformAdminPolicy};
use crate::ports::{
    AuditRepository, BlobStore, CatalogRepository, ConversationRepository, DirectoryRepository,
    ResourceRepository, RunExecutor, RunRepository, RuntimeProbe,
};
use crate::runtime_catalog::{BuiltinRuntimeDeclaration, RuntimeConnection};
use crate::time_utils::now;

/// 服务运行参数。
#[derive(Clone, Debug)]
pub struct AgentServiceConfig {
    /// 部署配置中的平台管理员 ID；不自动获得组织 scope。
    pub platform_admin_ids: Vec<i64>,
    /// 单个用户资源最大字节数。
    pub max_resource_bytes: usize,
    /// 单个 Run 产物最大字节数。
    pub max_artifact_bytes: usize,
    /// 部署声明的内置 Runtime。
    pub builtin_runtimes: Vec<BuiltinRuntimeDeclaration>,
}

/// Agent 服务依赖。
pub struct AgentServiceDependencies {
    /// 组织目录。
    pub directory: Arc<dyn DirectoryRepository>,
    /// Runtime/Agent/Skill 目录。
    pub catalog: Arc<dyn CatalogRepository>,
    /// Session 与消息。
    pub conversations: Arc<dyn ConversationRepository>,
    /// Canonical Run 与事件。
    pub runs: Arc<dyn RunRepository>,
    /// 用户资源与产物。
    pub resources: Arc<dyn ResourceRepository>,
    /// 审计记录。
    pub audit: Arc<dyn AuditRepository>,
    /// Blob 存储。
    pub blobs: Arc<dyn BlobStore>,
    /// Runtime 执行器。
    pub executor: Arc<dyn RunExecutor>,
    /// 外部 Runtime 探测。
    pub runtime_probe: Arc<dyn RuntimeProbe>,
    /// public 用户身份校验。
    pub users: Arc<dyn UserIdentityValidator>,
    /// 管理员身份校验。
    pub admins: Arc<dyn AdminIdentityValidator>,
    /// 服务运行参数。
    pub config: AgentServiceConfig,
}

/// Agent 服务用例入口。
pub struct AgentService {
    pub(super) deps: Arc<AgentServiceDependencies>,
    pub(super) platform_admins: PlatformAdminPolicy,
}

impl AgentService {
    /// 组装服务。
    pub fn new(deps: AgentServiceDependencies) -> Self {
        let platform_admins = PlatformAdminPolicy::new(deps.config.platform_admin_ids.clone());
        Self {
            deps: Arc::new(deps),
            platform_admins,
        }
    }

    /// 是否为部署配置的平台管理员；平台管理员不自动获得组织 scope。
    pub fn is_platform_admin(&self, admin: &AdminIdentity) -> bool {
        self.platform_admins.is_platform_admin(admin)
    }

    /// 启动时校验并幂等对齐内置 Runtime，再收敛遗留 Run。
    ///
    /// # Errors
    ///
    /// 声明非法、目录写入失败或恢复遗留 Run 失败时返回错误。
    pub async fn initialize(&self) -> Result<(), AgentError> {
        let mut names = BTreeSet::new();
        for declaration in &self.deps.config.builtin_runtimes {
            declaration.validate()?;
            if !names.insert(declaration.name.clone()) {
                return Err(AgentError::InvalidRequest(
                    "内置 Runtime 声明名称重复".to_owned(),
                ));
            }
        }
        self.deps
            .catalog
            .sync_builtin_runtimes(&self.deps.config.builtin_runtimes)
            .await?;
        let recovered = self.deps.runs.recover_incomplete_runs(now()).await?;
        if recovered > 0 {
            tracing::warn!(recovered, "启动时收敛了遗留 Run");
        }
        Ok(())
    }

    pub(super) async fn membership(
        &self,
        organization_id: Uuid,
        user_id: i64,
    ) -> Result<OrganizationMembership, AgentError> {
        self.deps
            .directory
            .get_membership(organization_id, user_id)
            .await?
            .ok_or(AgentError::Forbidden("organization_membership_required"))
    }

    pub(super) async fn agent_bundle(
        &self,
        agent_id: Uuid,
    ) -> Result<(BusinessAgent, RuntimeConnection, Vec<AgentAccessRule>), AgentError> {
        let agent = self
            .deps
            .catalog
            .get_agent(agent_id)
            .await?
            .ok_or(AgentError::NotFound("agent_not_found"))?;
        let runtime = self
            .deps
            .catalog
            .get_runtime(agent.runtime_connection_id)
            .await?
            .ok_or(AgentError::RuntimeUnavailable)?;
        let rules = self.deps.catalog.list_access_rules(agent_id).await?;
        Ok((agent, runtime, rules))
    }

    pub(super) async fn write_audit(&self, service_id: &str, entry: AuditEntry<'_>) {
        let record = NewAuditRecord {
            occurred_at: now(),
            service_id: service_id.to_owned(),
            admin_user_id: entry.admin.map(|admin| admin.admin_user_id),
            user_id: entry.user_id,
            organization_id: entry.organization_id,
            action: entry.action.to_owned(),
            target_id: entry.target_id,
            outcome: entry.outcome,
            detail: entry.detail,
        };
        if let Err(audit_error) = self.deps.audit.append(&record).await {
            tracing::warn!(
                error = %audit_error,
                action = entry.action,
                "写入审计记录失败"
            );
        }
    }
}

impl AgentServiceDependencies {
    /// 读取并冻结 Agent 的委派治理信息。
    ///
    /// # Errors
    ///
    /// 存储不可用、策略或子 Agent 数据不一致时返回错误。
    pub(super) async fn delegation_snapshot_for(
        &self,
        agent_id: Uuid,
        organization_id: Uuid,
    ) -> Result<Option<DelegationSnapshot>, AgentError> {
        let Some(policy) = self.catalog.get_delegation_policy(agent_id).await? else {
            return Ok(None);
        };
        let mut children = Vec::with_capacity(policy.child_agent_ids.len());
        for child_agent_id in &policy.child_agent_ids {
            if let Some(child) = self.catalog.get_agent(*child_agent_id).await?
                && child.organization_id == organization_id
            {
                children.push(DelegationChildRef {
                    agent_id: child.id,
                    name: child.name,
                    display_name: child.display_name,
                });
            }
        }
        delegation_snapshot(&policy, children).map(Some)
    }
}

/// 审计条目；只允许低敏关联信息。
pub(super) struct AuditEntry<'a> {
    pub(super) admin: Option<&'a AdminIdentity>,
    pub(super) user_id: Option<i64>,
    pub(super) organization_id: Option<Uuid>,
    pub(super) action: &'a str,
    pub(super) target_id: Option<String>,
    pub(super) outcome: AuditOutcome,
    pub(super) detail: Value,
}
