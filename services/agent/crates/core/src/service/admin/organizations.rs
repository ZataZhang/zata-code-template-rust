//! 组织、部门、成员、管理员 scope 与审计用例。

use super::*;

impl AgentService {
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
}
