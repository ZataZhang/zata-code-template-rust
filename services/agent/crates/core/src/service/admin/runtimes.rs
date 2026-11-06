//! 外部 Runtime 管理用例。

use super::*;

impl AgentService {
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
}
