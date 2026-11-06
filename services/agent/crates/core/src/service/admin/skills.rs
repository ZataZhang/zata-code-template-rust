//! Skill 与资产管理用例。

use super::*;

impl AgentService {
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
}
