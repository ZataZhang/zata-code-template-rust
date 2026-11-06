//! 组织管理员用例：组织/部门/成员/scope、Runtime、Agent、策略与 Skill 管理。

use serde_json::json;
use uuid::Uuid;

use super::{AgentService, AuditEntry};
use crate::agent_catalog::{
    AccessRuleInput, AgentAccessRule, AgentUpdate, BusinessAgent, NewAgent, validate_access_rules,
};
use crate::audit::AuditOutcome;
use crate::delegation::{DelegationPolicy, DelegationPolicyUpdate, validate_policy};
use crate::error::AgentError;
use crate::hash::sha256_hex;
use crate::identity::{AdminIdentity, ServicePrincipal};
use crate::organization::{
    AdminScope, Organization, OrganizationMembership, OrganizationUnit, validate_display_name,
    validate_slug, validate_text, validate_unit_assignment,
};
use crate::resources::{DownloadedBlob, UploadedFile, sanitize_filename, validate_resource_upload};
use crate::routing::EntryPolicy;
use crate::runtime_catalog::{
    NewExternalRuntime, RuntimeAdapterKind, RuntimeConfig, RuntimeConnection, RuntimeOrigin,
    RuntimeStatus, ensure_bindable, normalize_endpoint_url,
};
use crate::skill_access::{NewSkill, Skill, SkillAsset, SkillUpdate};
use crate::time_utils::now;

mod agents;
mod organizations;
mod runtimes;
mod skills;

impl AgentService {
    fn require_platform_admin(&self, admin: &AdminIdentity) -> Result<(), AgentError> {
        self.platform_admins.require_platform_admin(admin)
    }

    async fn scoped_organization(
        &self,
        admin: &AdminIdentity,
        organization_id: Uuid,
    ) -> Result<Organization, AgentError> {
        let organization = self
            .deps
            .directory
            .get_organization(organization_id)
            .await?
            .ok_or(AgentError::NotFound("organization_not_found"))?;
        let scope = self
            .deps
            .directory
            .get_admin_scope(organization_id, admin.admin_user_id)
            .await?;
        if scope.is_none() {
            return Err(AgentError::Forbidden("organization_scope_denied"));
        }
        Ok(organization)
    }

    async fn organization_agent(
        &self,
        admin: &AdminIdentity,
        organization_id: Uuid,
        agent_id: Uuid,
    ) -> Result<BusinessAgent, AgentError> {
        self.scoped_organization(admin, organization_id).await?;
        let agent = self
            .deps
            .catalog
            .get_agent(agent_id)
            .await?
            .ok_or(AgentError::NotFound("agent_not_found"))?;
        if agent.organization_id != organization_id {
            return Err(AgentError::NotFound("agent_not_found"));
        }
        Ok(agent)
    }

    async fn organization_skill(
        &self,
        admin: &AdminIdentity,
        organization_id: Uuid,
        skill_id: Uuid,
    ) -> Result<Skill, AgentError> {
        self.scoped_organization(admin, organization_id).await?;
        let skill = self
            .deps
            .catalog
            .get_skill(skill_id)
            .await?
            .ok_or(AgentError::NotFound("skill_not_found"))?;
        if skill.organization_id != organization_id {
            return Err(AgentError::NotFound("skill_not_found"));
        }
        Ok(skill)
    }
}
