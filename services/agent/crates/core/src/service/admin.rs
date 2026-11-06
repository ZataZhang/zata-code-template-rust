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

    /// 创建组织；仅平台管理员可执行，创建者不会自动获得组织 scope。
    ///
    /// # Errors
    ///
    /// 非平台管理员、slug 冲突或字段非法时返回错误。
    pub async fn create_organization(
        &self,
        principal: &ServicePrincipal,
        admin: &AdminIdentity,
        slug: &str,
        display_name: &str,
    ) -> Result<Organization, AgentError> {
        self.require_platform_admin(admin)?;
        validate_slug(slug)?;
        validate_display_name(display_name)?;
        if self
            .deps
            .directory
            .find_organization_by_slug(slug)
            .await?
            .is_some()
        {
            return Err(AgentError::Conflict("organization_slug_taken"));
        }
        let organization = Organization {
            id: Uuid::new_v4(),
            slug: slug.to_owned(),
            display_name: display_name.to_owned(),
            created_at: now(),
        };
        self.deps
            .directory
            .create_organization(&organization, admin.admin_user_id)
            .await?;
        self.write_audit(
            &principal.service_id,
            AuditEntry {
                admin: Some(admin),
                user_id: None,
                organization_id: Some(organization.id),
                action: "organization.create",
                target_id: Some(organization.slug.clone()),
                outcome: AuditOutcome::Succeeded,
                detail: json!({}),
            },
        )
        .await;
        Ok(organization)
    }

    /// 列出调用者可管理的组织。
    ///
    /// # Errors
    ///
    /// 存储不可用时返回错误。
    pub async fn list_organizations(
        &self,
        admin: &AdminIdentity,
    ) -> Result<Vec<Organization>, AgentError> {
        if self.platform_admins.is_platform_admin(admin) {
            return self.deps.directory.list_organizations().await;
        }
        let scopes = self
            .deps
            .directory
            .list_scopes_for_admin(admin.admin_user_id)
            .await?;
        let mut organizations = Vec::with_capacity(scopes.len());
        for scope in scopes {
            if let Some(organization) = self
                .deps
                .directory
                .get_organization(scope.organization_id)
                .await?
            {
                organizations.push(organization);
            }
        }
        Ok(organizations)
    }

    /// 读取组织。
    ///
    /// # Errors
    ///
    /// 组织不存在或没有 scope 时返回错误。
    pub async fn get_organization(
        &self,
        admin: &AdminIdentity,
        organization_id: Uuid,
    ) -> Result<Organization, AgentError> {
        self.scoped_organization(admin, organization_id).await
    }

    /// 创建部门。
    ///
    /// # Errors
    ///
    /// 名称冲突或字段非法时返回错误。
    pub async fn create_unit(
        &self,
        principal: &ServicePrincipal,
        admin: &AdminIdentity,
        organization_id: Uuid,
        name: &str,
    ) -> Result<OrganizationUnit, AgentError> {
        self.scoped_organization(admin, organization_id).await?;
        validate_text(name, 1, 128, "部门名称")?;
        let units = self.deps.directory.list_units(organization_id).await?;
        if units.iter().any(|unit| unit.name == name) {
            return Err(AgentError::Conflict("unit_name_taken"));
        }
        let unit = OrganizationUnit {
            id: Uuid::new_v4(),
            organization_id,
            name: name.to_owned(),
            created_at: now(),
        };
        self.deps.directory.create_unit(&unit).await?;
        self.write_audit(
            &principal.service_id,
            AuditEntry {
                admin: Some(admin),
                user_id: None,
                organization_id: Some(organization_id),
                action: "unit.create",
                target_id: Some(unit.id.to_string()),
                outcome: AuditOutcome::Succeeded,
                detail: json!({}),
            },
        )
        .await;
        Ok(unit)
    }

    /// 列出部门。
    ///
    /// # Errors
    ///
    /// 没有 scope 或存储不可用时返回错误。
    pub async fn list_units(
        &self,
        admin: &AdminIdentity,
        organization_id: Uuid,
    ) -> Result<Vec<OrganizationUnit>, AgentError> {
        self.scoped_organization(admin, organization_id).await?;
        self.deps.directory.list_units(organization_id).await
    }

    /// 删除部门。
    ///
    /// # Errors
    ///
    /// 部门仍有成员或没有 scope 时返回错误。
    pub async fn delete_unit(
        &self,
        principal: &ServicePrincipal,
        admin: &AdminIdentity,
        organization_id: Uuid,
        unit_id: Uuid,
    ) -> Result<(), AgentError> {
        self.scoped_organization(admin, organization_id).await?;
        self.deps
            .directory
            .delete_unit(organization_id, unit_id)
            .await?;
        self.write_audit(
            &principal.service_id,
            AuditEntry {
                admin: Some(admin),
                user_id: None,
                organization_id: Some(organization_id),
                action: "unit.delete",
                target_id: Some(unit_id.to_string()),
                outcome: AuditOutcome::Succeeded,
                detail: json!({}),
            },
        )
        .await;
        Ok(())
    }

    /// 新增或更新显式成员关系。
    ///
    /// # Errors
    ///
    /// 用户 ID 非法、部门跨组织或没有 scope 时返回错误。
    pub async fn upsert_membership(
        &self,
        principal: &ServicePrincipal,
        admin: &AdminIdentity,
        organization_id: Uuid,
        user_id: i64,
        unit_id: Option<Uuid>,
        display_name: Option<&str>,
    ) -> Result<OrganizationMembership, AgentError> {
        self.scoped_organization(admin, organization_id).await?;
        if user_id <= 0 {
            return Err(AgentError::InvalidRequest(
                "用户 ID 必须为正整数".to_owned(),
            ));
        }
        let units = self.deps.directory.list_units(organization_id).await?;
        validate_unit_assignment(&units, organization_id, unit_id)?;
        if let Some(display_name) = display_name {
            validate_text(display_name, 1, 200, "成员展示名称")?;
        }
        let membership = OrganizationMembership {
            organization_id,
            user_id,
            unit_id,
            display_name: display_name.map(str::to_owned),
            created_at: now(),
        };
        self.deps.directory.upsert_membership(&membership).await?;
        self.write_audit(
            &principal.service_id,
            AuditEntry {
                admin: Some(admin),
                user_id: Some(user_id),
                organization_id: Some(organization_id),
                action: "membership.upsert",
                target_id: Some(user_id.to_string()),
                outcome: AuditOutcome::Succeeded,
                detail: json!({"unit_id": unit_id}),
            },
        )
        .await;
        Ok(membership)
    }

    /// 列出组织成员。
    ///
    /// # Errors
    ///
    /// 没有 scope 或存储不可用时返回错误。
    pub async fn list_memberships(
        &self,
        admin: &AdminIdentity,
        organization_id: Uuid,
    ) -> Result<Vec<OrganizationMembership>, AgentError> {
        self.scoped_organization(admin, organization_id).await?;
        self.deps.directory.list_memberships(organization_id).await
    }

    /// 删除成员关系。
    ///
    /// # Errors
    ///
    /// 没有 scope 或存储不可用时返回错误。
    pub async fn remove_membership(
        &self,
        principal: &ServicePrincipal,
        admin: &AdminIdentity,
        organization_id: Uuid,
        user_id: i64,
    ) -> Result<(), AgentError> {
        self.scoped_organization(admin, organization_id).await?;
        self.deps
            .directory
            .remove_membership(organization_id, user_id)
            .await?;
        self.write_audit(
            &principal.service_id,
            AuditEntry {
                admin: Some(admin),
                user_id: Some(user_id),
                organization_id: Some(organization_id),
                action: "membership.remove",
                target_id: Some(user_id.to_string()),
                outcome: AuditOutcome::Succeeded,
                detail: json!({}),
            },
        )
        .await;
        Ok(())
    }

    /// 列出组织管理员 scope。
    ///
    /// # Errors
    ///
    /// 没有 scope 时返回错误。
    pub async fn list_admin_scopes(
        &self,
        admin: &AdminIdentity,
        organization_id: Uuid,
    ) -> Result<Vec<AdminScope>, AgentError> {
        self.scoped_organization(admin, organization_id).await?;
        self.deps.directory.list_admin_scopes(organization_id).await
    }

    /// 授予管理员组织 scope；仅平台管理员可执行。
    ///
    /// # Errors
    ///
    /// 非平台管理员、组织不存在或目标 ID 非法时返回错误。
    pub async fn grant_admin_scope(
        &self,
        principal: &ServicePrincipal,
        admin: &AdminIdentity,
        organization_id: Uuid,
        target_admin_id: i64,
    ) -> Result<AdminScope, AgentError> {
        self.require_platform_admin(admin)?;
        if self
            .deps
            .directory
            .get_organization(organization_id)
            .await?
            .is_none()
        {
            return Err(AgentError::NotFound("organization_not_found"));
        }
        if target_admin_id <= 0 {
            return Err(AgentError::InvalidRequest(
                "管理员 ID 必须为正整数".to_owned(),
            ));
        }
        let scope = AdminScope {
            organization_id,
            admin_user_id: target_admin_id,
            granted_by_admin_id: admin.admin_user_id,
            created_at: now(),
        };
        self.deps.directory.grant_admin_scope(&scope).await?;
        self.write_audit(
            &principal.service_id,
            AuditEntry {
                admin: Some(admin),
                user_id: None,
                organization_id: Some(organization_id),
                action: "admin_scope.grant",
                target_id: Some(target_admin_id.to_string()),
                outcome: AuditOutcome::Succeeded,
                detail: json!({}),
            },
        )
        .await;
        Ok(scope)
    }

    /// 撤销管理员组织 scope；仅平台管理员可执行。
    ///
    /// # Errors
    ///
    /// 非平台管理员时返回错误。
    pub async fn revoke_admin_scope(
        &self,
        principal: &ServicePrincipal,
        admin: &AdminIdentity,
        organization_id: Uuid,
        target_admin_id: i64,
    ) -> Result<(), AgentError> {
        self.require_platform_admin(admin)?;
        self.deps
            .directory
            .revoke_admin_scope(organization_id, target_admin_id)
            .await?;
        self.write_audit(
            &principal.service_id,
            AuditEntry {
                admin: Some(admin),
                user_id: None,
                organization_id: Some(organization_id),
                action: "admin_scope.revoke",
                target_id: Some(target_admin_id.to_string()),
                outcome: AuditOutcome::Succeeded,
                detail: json!({}),
            },
        )
        .await;
        Ok(())
    }

    /// 列出组织可见的内置与外部 Runtime。
    ///
    /// # Errors
    ///
    /// 没有 scope 或存储不可用时返回错误。
    pub async fn list_runtimes(
        &self,
        admin: &AdminIdentity,
        organization_id: Uuid,
    ) -> Result<Vec<RuntimeConnection>, AgentError> {
        self.scoped_organization(admin, organization_id).await?;
        self.deps
            .catalog
            .list_runtimes_for_organization(organization_id)
            .await
    }

    /// 为组织登记外部 A2A Runtime 并执行 Card 探测。
    ///
    /// # Errors
    ///
    /// 字段非法、名称冲突或没有 scope 时返回错误；探测失败会保存为 unavailable。
    pub async fn create_external_runtime(
        &self,
        principal: &ServicePrincipal,
        admin: &AdminIdentity,
        organization_id: Uuid,
        input: NewExternalRuntime,
    ) -> Result<RuntimeConnection, AgentError> {
        self.scoped_organization(admin, organization_id).await?;
        input.validate()?;
        let endpoint_url = normalize_endpoint_url(&input.endpoint_url)?;
        if self
            .deps
            .catalog
            .find_runtime_by_name(Some(organization_id), &input.name)
            .await?
            .is_some()
        {
            return Err(AgentError::Conflict("runtime_name_taken"));
        }
        let probe = self
            .deps
            .runtime_probe
            .probe(&endpoint_url, input.credential_alias.as_deref())
            .await;
        let (status, status_detail) = match probe {
            Ok(result) if result.ready => (
                RuntimeStatus::Ready,
                result
                    .card_name
                    .map(|card_name| format!("Agent Card: {card_name}")),
            ),
            Ok(result) => (RuntimeStatus::Unavailable, result.status_detail),
            Err(error) => {
                tracing::warn!(error = %error, "A2A Agent Card 探测失败");
                (
                    RuntimeStatus::Unavailable,
                    Some("Agent Card 探测失败".to_owned()),
                )
            }
        };
        let current_time = now();
        let runtime = RuntimeConnection {
            id: Uuid::new_v4(),
            organization_id: Some(organization_id),
            name: input.name,
            display_name: input.display_name,
            origin: RuntimeOrigin::External,
            adapter_kind: RuntimeAdapterKind::A2a,
            endpoint_url: Some(endpoint_url),
            credential_alias: input.credential_alias,
            status,
            status_detail,
            config: RuntimeConfig {
                allowed_tools: Vec::new(),
                max_steps: 8,
                a2a_protocol_version: Some("1.0".to_owned()),
            },
            created_at: current_time,
            updated_at: current_time,
        };
        self.deps.catalog.insert_external_runtime(&runtime).await?;
        self.write_audit(
            &principal.service_id,
            AuditEntry {
                admin: Some(admin),
                user_id: None,
                organization_id: Some(organization_id),
                action: "runtime.create",
                target_id: Some(runtime.id.to_string()),
                outcome: AuditOutcome::Succeeded,
                detail: json!({"origin": "external", "adapter_kind": "a2a"}),
            },
        )
        .await;
        Ok(runtime)
    }

    /// 重新探测外部 Runtime 并更新状态。
    ///
    /// # Errors
    ///
    /// 内置 Runtime、跨组织或存储失败时返回错误。
    pub async fn refresh_runtime(
        &self,
        principal: &ServicePrincipal,
        admin: &AdminIdentity,
        organization_id: Uuid,
        runtime_id: Uuid,
    ) -> Result<RuntimeConnection, AgentError> {
        self.scoped_organization(admin, organization_id).await?;
        let mut runtime = self
            .deps
            .catalog
            .get_runtime(runtime_id)
            .await?
            .ok_or(AgentError::NotFound("runtime_not_found"))?;
        if runtime.origin != RuntimeOrigin::External
            || runtime.organization_id != Some(organization_id)
        {
            return Err(AgentError::Forbidden("builtin_runtime_read_only"));
        }
        let endpoint_url = runtime
            .endpoint_url
            .clone()
            .ok_or(AgentError::RuntimeUnavailable)?;
        let probe = self
            .deps
            .runtime_probe
            .probe(&endpoint_url, runtime.credential_alias.as_deref())
            .await;
        match probe {
            Ok(result) => {
                runtime.status = if result.ready {
                    RuntimeStatus::Ready
                } else {
                    RuntimeStatus::Unavailable
                };
                runtime.status_detail = if result.ready {
                    result
                        .card_name
                        .map(|card_name| format!("Agent Card: {card_name}"))
                } else {
                    result.status_detail
                };
            }
            Err(error) => {
                tracing::warn!(error = %error, "A2A Agent Card 探测失败");
                runtime.status = RuntimeStatus::Unavailable;
                runtime.status_detail = Some("Agent Card 探测失败".to_owned());
            }
        }
        runtime.updated_at = now();
        self.deps.catalog.update_runtime(&runtime).await?;
        self.write_audit(
            &principal.service_id,
            AuditEntry {
                admin: Some(admin),
                user_id: None,
                organization_id: Some(organization_id),
                action: "runtime.refresh",
                target_id: Some(runtime.id.to_string()),
                outcome: AuditOutcome::Succeeded,
                detail: json!({"status": runtime.status.as_str()}),
            },
        )
        .await;
        Ok(runtime)
    }

    /// 列出组织 Agent。
    ///
    /// # Errors
    ///
    /// 没有 scope 时返回错误。
    pub async fn list_agents(
        &self,
        admin: &AdminIdentity,
        organization_id: Uuid,
    ) -> Result<Vec<BusinessAgent>, AgentError> {
        self.scoped_organization(admin, organization_id).await?;
        self.deps.catalog.list_agents(organization_id).await
    }

    /// 读取组织 Agent。
    ///
    /// # Errors
    ///
    /// Agent 不存在或跨组织时返回错误。
    pub async fn get_agent(
        &self,
        admin: &AdminIdentity,
        organization_id: Uuid,
        agent_id: Uuid,
    ) -> Result<BusinessAgent, AgentError> {
        self.organization_agent(admin, organization_id, agent_id)
            .await
    }

    /// 创建 Agent，并在创建时固定 Runtime binding。
    ///
    /// # Errors
    ///
    /// 字段非法、名称冲突、Runtime 不属于组织或不可用时返回错误。
    pub async fn create_agent(
        &self,
        principal: &ServicePrincipal,
        admin: &AdminIdentity,
        organization_id: Uuid,
        new_agent: NewAgent,
    ) -> Result<BusinessAgent, AgentError> {
        self.scoped_organization(admin, organization_id).await?;
        if new_agent.organization_id != organization_id {
            return Err(AgentError::InvalidRequest(
                "Agent 组织与路径不一致".to_owned(),
            ));
        }
        new_agent.validate()?;
        let runtime = self
            .deps
            .catalog
            .get_runtime(new_agent.runtime_connection_id)
            .await?
            .ok_or(AgentError::NotFound("runtime_not_found"))?;
        ensure_bindable(organization_id, &runtime)?;
        let agents = self.deps.catalog.list_agents(organization_id).await?;
        if agents.iter().any(|agent| agent.name == new_agent.name) {
            return Err(AgentError::Conflict("agent_name_taken"));
        }
        let current_time = now();
        let mut agent = BusinessAgent {
            id: Uuid::new_v4(),
            organization_id,
            runtime_connection_id: new_agent.runtime_connection_id,
            name: new_agent.name,
            display_name: new_agent.display_name,
            description: new_agent.description,
            prompt: new_agent.prompt,
            owner_admin_id: admin.admin_user_id,
            enabled: true,
            descriptor_checksum: String::new(),
            created_at: current_time,
            updated_at: current_time,
        };
        agent.descriptor_checksum = agent.compute_checksum()?;
        self.deps.catalog.insert_agent(&agent, &[]).await?;
        self.write_audit(
            &principal.service_id,
            AuditEntry {
                admin: Some(admin),
                user_id: None,
                organization_id: Some(organization_id),
                action: "agent.create",
                target_id: Some(agent.id.to_string()),
                outcome: AuditOutcome::Succeeded,
                detail: json!({"runtime_connection_id": agent.runtime_connection_id}),
            },
        )
        .await;
        Ok(agent)
    }

    /// 更新 Agent descriptor；Runtime binding 保持不变。
    ///
    /// # Errors
    ///
    /// 字段非法、Agent 不存在或跨组织时返回错误。
    pub async fn update_agent(
        &self,
        principal: &ServicePrincipal,
        admin: &AdminIdentity,
        organization_id: Uuid,
        agent_id: Uuid,
        update: AgentUpdate,
    ) -> Result<BusinessAgent, AgentError> {
        let mut agent = self
            .organization_agent(admin, organization_id, agent_id)
            .await?;
        update.validate()?;
        agent.display_name = update.display_name;
        agent.description = update.description;
        agent.prompt = update.prompt;
        agent.enabled = update.enabled;
        agent.updated_at = now();
        agent.descriptor_checksum = agent.compute_checksum()?;
        self.deps.catalog.update_agent(&agent).await?;
        self.write_audit(
            &principal.service_id,
            AuditEntry {
                admin: Some(admin),
                user_id: None,
                organization_id: Some(organization_id),
                action: "agent.update",
                target_id: Some(agent.id.to_string()),
                outcome: AuditOutcome::Succeeded,
                detail: json!({"enabled": agent.enabled}),
            },
        )
        .await;
        Ok(agent)
    }

    /// 替换 Agent 访问规则。
    ///
    /// # Errors
    ///
    /// 规则非法、部门跨组织或 Agent 不存在时返回错误。
    pub async fn replace_agent_access_rules(
        &self,
        principal: &ServicePrincipal,
        admin: &AdminIdentity,
        organization_id: Uuid,
        agent_id: Uuid,
        rules: Vec<AccessRuleInput>,
    ) -> Result<Vec<AgentAccessRule>, AgentError> {
        let agent = self
            .organization_agent(admin, organization_id, agent_id)
            .await?;
        validate_access_rules(&agent, &rules)?;
        let units = self.deps.directory.list_units(organization_id).await?;
        for rule in &rules {
            if let crate::agent_catalog::AccessSubject::Unit(unit_id) = rule.subject {
                validate_unit_assignment(&units, organization_id, Some(unit_id))?;
            }
        }
        let created_at = now();
        let stored_rules = rules
            .into_iter()
            .map(|rule| AgentAccessRule {
                id: Uuid::new_v4(),
                agent_id,
                effect: rule.effect,
                subject: rule.subject,
                created_at,
            })
            .collect::<Vec<_>>();
        self.deps
            .catalog
            .replace_access_rules(agent_id, &stored_rules)
            .await?;
        self.write_audit(
            &principal.service_id,
            AuditEntry {
                admin: Some(admin),
                user_id: None,
                organization_id: Some(organization_id),
                action: "agent.access_rules.replace",
                target_id: Some(agent_id.to_string()),
                outcome: AuditOutcome::Succeeded,
                detail: json!({"rule_count": stored_rules.len()}),
            },
        )
        .await;
        Ok(stored_rules)
    }

    /// 新增或更新 Agent 入口策略。
    ///
    /// # Errors
    ///
    /// 优先级越界或 Agent 不存在时返回错误。
    pub async fn upsert_entry_policy(
        &self,
        principal: &ServicePrincipal,
        admin: &AdminIdentity,
        organization_id: Uuid,
        agent_id: Uuid,
        auto_visible: bool,
        priority: i32,
    ) -> Result<EntryPolicy, AgentError> {
        let agent = self
            .organization_agent(admin, organization_id, agent_id)
            .await?;
        if !(0..=10_000).contains(&priority) {
            return Err(AgentError::InvalidRequest(
                "入口策略优先级必须在 0-10000 之间".to_owned(),
            ));
        }
        let policy = EntryPolicy {
            agent_id,
            organization_id: agent.organization_id,
            auto_visible,
            priority,
            updated_at: now(),
        };
        self.deps.catalog.upsert_entry_policy(&policy).await?;
        self.write_audit(
            &principal.service_id,
            AuditEntry {
                admin: Some(admin),
                user_id: None,
                organization_id: Some(organization_id),
                action: "agent.entry_policy.upsert",
                target_id: Some(agent_id.to_string()),
                outcome: AuditOutcome::Succeeded,
                detail: json!({"auto_visible": auto_visible, "priority": priority}),
            },
        )
        .await;
        Ok(policy)
    }

    /// 新增或更新 Agent 委派策略。
    ///
    /// # Errors
    ///
    /// 子 Agent 跨组织、策略越界或自委派时返回错误。
    pub async fn upsert_delegation_policy(
        &self,
        principal: &ServicePrincipal,
        admin: &AdminIdentity,
        organization_id: Uuid,
        agent_id: Uuid,
        update: DelegationPolicyUpdate,
    ) -> Result<DelegationPolicy, AgentError> {
        self.organization_agent(admin, organization_id, agent_id)
            .await?;
        let policy = DelegationPolicy {
            parent_agent_id: agent_id,
            child_agent_ids: update.child_agent_ids,
            max_depth: update.max_depth,
            max_child_runs: update.max_child_runs,
        };
        validate_policy(&policy)?;
        for child_agent_id in &policy.child_agent_ids {
            let child = self
                .deps
                .catalog
                .get_agent(*child_agent_id)
                .await?
                .ok_or(AgentError::NotFound("child_agent_not_found"))?;
            if child.organization_id != organization_id {
                return Err(AgentError::InvalidRequest(
                    "委派子 Agent 必须属于同一组织".to_owned(),
                ));
            }
        }
        self.deps.catalog.upsert_delegation_policy(&policy).await?;
        self.write_audit(
            &principal.service_id,
            AuditEntry {
                admin: Some(admin),
                user_id: None,
                organization_id: Some(organization_id),
                action: "agent.delegation_policy.upsert",
                target_id: Some(agent_id.to_string()),
                outcome: AuditOutcome::Succeeded,
                detail: json!({
                    "child_count": policy.child_agent_ids.len(),
                    "max_depth": policy.max_depth,
                    "max_child_runs": policy.max_child_runs,
                }),
            },
        )
        .await;
        Ok(policy)
    }

    /// 列出组织 Skill。
    ///
    /// # Errors
    ///
    /// 没有 scope 时返回错误。
    pub async fn list_skills(
        &self,
        admin: &AdminIdentity,
        organization_id: Uuid,
    ) -> Result<Vec<Skill>, AgentError> {
        self.scoped_organization(admin, organization_id).await?;
        self.deps.catalog.list_skills(organization_id).await
    }

    /// 创建 Skill。
    ///
    /// # Errors
    ///
    /// 名称冲突或字段非法时返回错误。
    pub async fn create_skill(
        &self,
        principal: &ServicePrincipal,
        admin: &AdminIdentity,
        organization_id: Uuid,
        new_skill: NewSkill,
    ) -> Result<Skill, AgentError> {
        self.scoped_organization(admin, organization_id).await?;
        new_skill.validate()?;
        let skills = self.deps.catalog.list_skills(organization_id).await?;
        if skills.iter().any(|skill| skill.name == new_skill.name) {
            return Err(AgentError::Conflict("skill_name_taken"));
        }
        let current_time = now();
        let skill = Skill {
            id: Uuid::new_v4(),
            organization_id,
            name: new_skill.name,
            description: new_skill.description,
            instructions: new_skill.instructions,
            enabled: true,
            created_at: current_time,
            updated_at: current_time,
        };
        self.deps.catalog.insert_skill(&skill).await?;
        self.write_audit(
            &principal.service_id,
            AuditEntry {
                admin: Some(admin),
                user_id: None,
                organization_id: Some(organization_id),
                action: "skill.create",
                target_id: Some(skill.id.to_string()),
                outcome: AuditOutcome::Succeeded,
                detail: json!({}),
            },
        )
        .await;
        Ok(skill)
    }

    /// 更新 Skill。
    ///
    /// # Errors
    ///
    /// Skill 不存在或字段非法时返回错误。
    pub async fn update_skill(
        &self,
        principal: &ServicePrincipal,
        admin: &AdminIdentity,
        organization_id: Uuid,
        skill_id: Uuid,
        update: SkillUpdate,
    ) -> Result<Skill, AgentError> {
        let mut skill = self
            .organization_skill(admin, organization_id, skill_id)
            .await?;
        update.validate()?;
        skill.description = update.description;
        skill.instructions = update.instructions;
        skill.enabled = update.enabled;
        skill.updated_at = now();
        self.deps.catalog.update_skill(&skill).await?;
        self.write_audit(
            &principal.service_id,
            AuditEntry {
                admin: Some(admin),
                user_id: None,
                organization_id: Some(organization_id),
                action: "skill.update",
                target_id: Some(skill.id.to_string()),
                outcome: AuditOutcome::Succeeded,
                detail: json!({"enabled": skill.enabled}),
            },
        )
        .await;
        Ok(skill)
    }

    /// 替换 Agent 可选择的 Skill。
    ///
    /// # Errors
    ///
    /// Skill 跨组织、重复或 Agent 不存在时返回错误。
    pub async fn replace_agent_skills(
        &self,
        principal: &ServicePrincipal,
        admin: &AdminIdentity,
        organization_id: Uuid,
        agent_id: Uuid,
        skill_ids: Vec<Uuid>,
    ) -> Result<Vec<Skill>, AgentError> {
        self.organization_agent(admin, organization_id, agent_id)
            .await?;
        let mut unique_skill_ids = skill_ids.clone();
        unique_skill_ids.sort_unstable();
        unique_skill_ids.dedup();
        if unique_skill_ids.len() != skill_ids.len() {
            return Err(AgentError::InvalidRequest(
                "Skill 授权包含重复项".to_owned(),
            ));
        }
        let skills = self.deps.catalog.list_skills(organization_id).await?;
        for skill_id in &unique_skill_ids {
            if !skills.iter().any(|skill| skill.id == *skill_id) {
                return Err(AgentError::NotFound("skill_not_found"));
            }
        }
        self.deps
            .catalog
            .replace_skill_assignments(agent_id, &unique_skill_ids)
            .await?;
        self.write_audit(
            &principal.service_id,
            AuditEntry {
                admin: Some(admin),
                user_id: None,
                organization_id: Some(organization_id),
                action: "agent.skills.replace",
                target_id: Some(agent_id.to_string()),
                outcome: AuditOutcome::Succeeded,
                detail: json!({"skill_count": unique_skill_ids.len()}),
            },
        )
        .await;
        Ok(skills
            .into_iter()
            .filter(|skill| unique_skill_ids.contains(&skill.id))
            .collect())
    }

    /// 列出 Skill 资产元数据。
    ///
    /// # Errors
    ///
    /// Skill 不存在或没有 scope 时返回错误。
    pub async fn list_skill_assets(
        &self,
        admin: &AdminIdentity,
        organization_id: Uuid,
        skill_id: Uuid,
    ) -> Result<Vec<SkillAsset>, AgentError> {
        self.organization_skill(admin, organization_id, skill_id)
            .await?;
        let stored = self.deps.catalog.list_skill_assets(&[skill_id]).await?;
        Ok(stored.into_iter().map(|entry| entry.asset).collect())
    }

    /// 上传 Skill 资产。
    ///
    /// # Errors
    ///
    /// 文件类型/大小非法、Skill 不存在或 Blob 写入失败时返回错误。
    pub async fn upload_skill_asset(
        &self,
        principal: &ServicePrincipal,
        admin: &AdminIdentity,
        organization_id: Uuid,
        skill_id: Uuid,
        file: UploadedFile,
    ) -> Result<SkillAsset, AgentError> {
        self.organization_skill(admin, organization_id, skill_id)
            .await?;
        validate_resource_upload(
            &file.filename,
            &file.media_type,
            file.bytes.len(),
            self.deps.config.max_resource_bytes,
        )?;
        let filename = sanitize_filename(&file.filename)?;
        let asset_id = Uuid::new_v4();
        let storage_key = format!("org/{organization_id}/skills/{skill_id}/assets/{asset_id}");
        let sha256 = sha256_hex(&file.bytes);
        self.deps.blobs.put(&storage_key, &file.bytes).await?;
        let asset = SkillAsset {
            id: asset_id,
            skill_id,
            name: filename,
            media_type: file.media_type,
            size_bytes: i64::try_from(file.bytes.len()).map_err(|_| AgentError::Internal)?,
            sha256,
        };
        if let Err(insert_error) = self
            .deps
            .catalog
            .insert_skill_asset(&asset, &storage_key)
            .await
        {
            let _ = self.deps.blobs.delete(&storage_key).await;
            return Err(insert_error);
        }
        self.write_audit(
            &principal.service_id,
            AuditEntry {
                admin: Some(admin),
                user_id: None,
                organization_id: Some(organization_id),
                action: "skill.asset.upload",
                target_id: Some(asset.id.to_string()),
                outcome: AuditOutcome::Succeeded,
                detail: json!({"skill_id": skill_id, "size": asset.size_bytes}),
            },
        )
        .await;
        Ok(asset)
    }

    /// 下载 Skill 资产。
    ///
    /// # Errors
    ///
    /// 资产不存在、跨 Skill 或 Blob 缺失时返回错误。
    pub async fn download_skill_asset(
        &self,
        admin: &AdminIdentity,
        organization_id: Uuid,
        skill_id: Uuid,
        asset_id: Uuid,
    ) -> Result<DownloadedBlob, AgentError> {
        self.organization_skill(admin, organization_id, skill_id)
            .await?;
        let stored = self
            .deps
            .catalog
            .get_skill_asset(asset_id)
            .await?
            .ok_or(AgentError::NotFound("asset_not_found"))?;
        if stored.asset.skill_id != skill_id {
            return Err(AgentError::NotFound("asset_not_found"));
        }
        let bytes = self
            .deps
            .blobs
            .get(&stored.storage_key)
            .await?
            .ok_or(AgentError::NotFound("asset_not_found"))?;
        Ok(DownloadedBlob {
            filename: stored.asset.name,
            media_type: stored.asset.media_type,
            bytes,
        })
    }

    /// 删除 Skill 资产。
    ///
    /// # Errors
    ///
    /// 资产不存在或跨 Skill 时返回错误。
    pub async fn delete_skill_asset(
        &self,
        principal: &ServicePrincipal,
        admin: &AdminIdentity,
        organization_id: Uuid,
        skill_id: Uuid,
        asset_id: Uuid,
    ) -> Result<(), AgentError> {
        self.organization_skill(admin, organization_id, skill_id)
            .await?;
        let stored = self
            .deps
            .catalog
            .get_skill_asset(asset_id)
            .await?
            .ok_or(AgentError::NotFound("asset_not_found"))?;
        if stored.asset.skill_id != skill_id {
            return Err(AgentError::NotFound("asset_not_found"));
        }
        self.deps.catalog.delete_skill_asset(asset_id).await?;
        let _ = self.deps.blobs.delete(&stored.storage_key).await;
        self.write_audit(
            &principal.service_id,
            AuditEntry {
                admin: Some(admin),
                user_id: None,
                organization_id: Some(organization_id),
                action: "skill.asset.delete",
                target_id: Some(asset_id.to_string()),
                outcome: AuditOutcome::Succeeded,
                detail: json!({"skill_id": skill_id}),
            },
        )
        .await;
        Ok(())
    }

    /// 列出组织审计记录。
    ///
    /// # Errors
    ///
    /// 没有 scope 时返回错误。
    pub async fn list_audit(
        &self,
        admin: &AdminIdentity,
        organization_id: Uuid,
    ) -> Result<Vec<crate::audit::AuditRecord>, AgentError> {
        self.scoped_organization(admin, organization_id).await?;
        self.deps.audit.list(Some(organization_id), 200).await
    }

    /// 读取 Agent 的访问规则，供管理面展示与编辑。
    ///
    /// # Errors
    ///
    /// Agent 不存在、跨组织或没有 scope 时返回错误。
    pub async fn list_agent_access_rules(
        &self,
        admin: &AdminIdentity,
        organization_id: Uuid,
        agent_id: Uuid,
    ) -> Result<Vec<AgentAccessRule>, AgentError> {
        self.organization_agent(admin, organization_id, agent_id)
            .await?;
        self.deps.catalog.list_access_rules(agent_id).await
    }

    /// 读取 Agent 入口策略；未配置时返回 `None`。
    ///
    /// # Errors
    ///
    /// Agent 不存在、跨组织或没有 scope 时返回错误。
    pub async fn get_agent_entry_policy(
        &self,
        admin: &AdminIdentity,
        organization_id: Uuid,
        agent_id: Uuid,
    ) -> Result<Option<EntryPolicy>, AgentError> {
        self.organization_agent(admin, organization_id, agent_id)
            .await?;
        self.deps.catalog.get_entry_policy(agent_id).await
    }

    /// 读取 Agent 委派策略；未配置时返回 `None`。
    ///
    /// # Errors
    ///
    /// Agent 不存在、跨组织或没有 scope 时返回错误。
    pub async fn get_agent_delegation_policy(
        &self,
        admin: &AdminIdentity,
        organization_id: Uuid,
        agent_id: Uuid,
    ) -> Result<Option<DelegationPolicy>, AgentError> {
        self.organization_agent(admin, organization_id, agent_id)
            .await?;
        self.deps.catalog.get_delegation_policy(agent_id).await
    }

    /// 列出 Agent 已授权的 Skill ID。
    ///
    /// # Errors
    ///
    /// Agent 不存在、跨组织或没有 scope 时返回错误。
    pub async fn list_agent_skill_ids(
        &self,
        admin: &AdminIdentity,
        organization_id: Uuid,
        agent_id: Uuid,
    ) -> Result<Vec<Uuid>, AgentError> {
        self.organization_agent(admin, organization_id, agent_id)
            .await?;
        self.deps.catalog.list_skill_assignments(agent_id).await
    }
}
