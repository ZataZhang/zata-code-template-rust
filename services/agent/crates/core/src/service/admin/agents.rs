//! Agent、访问规则与入口策略管理用例。

use super::*;

impl AgentService {
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
