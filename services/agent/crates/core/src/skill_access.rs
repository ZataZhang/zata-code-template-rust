//! Skill 可见性、Agent 授权与 Run 选择规则。
//!
//! Skill 机制属于通用能力；具体业务 Skill 内容由后续需求接入。
//! Run 只快照实际选择的 Skill 指令与资产摘要，后续 Skill 更新不改写历史 Run。

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::AgentError;
use crate::hash::sha256_json;
use crate::organization::validate_text;

/// 组织级 Skill。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skill {
    /// Skill UUID。
    pub id: Uuid,
    /// 所属组织。
    pub organization_id: Uuid,
    /// 组织内唯一名称。
    pub name: String,
    /// 展示说明。
    pub description: String,
    /// 装配进 Run 的指令。
    pub instructions: String,
    /// 是否允许新 Run 选择。
    pub enabled: bool,
    /// 创建时间。
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// 最近更新时间。
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// Skill 资产元数据。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillAsset {
    /// 资产 UUID。
    pub id: Uuid,
    /// 所属 Skill。
    pub skill_id: Uuid,
    /// 资产名称。
    pub name: String,
    /// MIME 类型。
    pub media_type: String,
    /// 字节数。
    pub size_bytes: i64,
    /// 内容 SHA-256。
    pub sha256: String,
}

/// Run 快照中的 Skill 资产引用。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillAssetSnapshot {
    /// 资产 UUID。
    pub asset_id: Uuid,
    /// 资产名称。
    pub name: String,
    /// MIME 类型。
    pub media_type: String,
    /// 字节数。
    pub size_bytes: i64,
    /// 内容 SHA-256。
    pub sha256: String,
}

/// Run 快照中的 Skill。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillSnapshot {
    /// Skill UUID。
    pub skill_id: Uuid,
    /// 名称。
    pub name: String,
    /// 指令。
    pub instructions: String,
    /// 资产引用。
    pub assets: Vec<SkillAssetSnapshot>,
    /// Skill 快照摘要。
    pub checksum: String,
}

/// 创建 Skill 的输入。
#[derive(Clone, Debug)]
pub struct NewSkill {
    /// 组织内名称。
    pub name: String,
    /// 展示说明。
    pub description: String,
    /// 指令。
    pub instructions: String,
}

impl NewSkill {
    /// 校验字段。
    ///
    /// # Errors
    ///
    /// 名称/指令为空、越界或含控制字符时返回错误。
    pub fn validate(&self) -> Result<(), AgentError> {
        validate_text(&self.name, 1, 128, "Skill 名称")?;
        if self.description.chars().count() > 2000 || self.description.chars().any(char::is_control)
        {
            return Err(AgentError::InvalidRequest(
                "Skill 描述不能超过 2000 字符或包含控制字符".to_owned(),
            ));
        }
        validate_text(&self.instructions, 1, 20000, "Skill 指令")
    }
}

/// 更新 Skill 的可变字段。
#[derive(Clone, Debug)]
pub struct SkillUpdate {
    /// 展示说明。
    pub description: String,
    /// 指令。
    pub instructions: String,
    /// 是否允许新 Run 选择。
    pub enabled: bool,
}

impl SkillUpdate {
    /// 校验字段。
    ///
    /// # Errors
    ///
    /// 描述或指令越界、含控制字符时返回错误。
    pub fn validate(&self) -> Result<(), AgentError> {
        if self.description.chars().count() > 2000 || self.description.chars().any(char::is_control)
        {
            return Err(AgentError::InvalidRequest(
                "Skill 描述不能超过 2000 字符或包含控制字符".to_owned(),
            ));
        }
        validate_text(&self.instructions, 1, 20000, "Skill 指令")
    }
}

/// 解析本次 Run 实际选择的 Skill。
///
/// # Errors
///
/// 请求重复、Skill 未分配给 Agent、已停用或跨组织时返回错误。
pub fn resolve_selected_skills(
    organization_id: Uuid,
    requested_skill_ids: &[Uuid],
    assigned_skill_ids: &[Uuid],
    skills: &[Skill],
    assets: &[SkillAsset],
) -> Result<Vec<SkillSnapshot>, AgentError> {
    let mut unique_requested = requested_skill_ids.to_vec();
    unique_requested.sort_unstable();
    unique_requested.dedup();
    if unique_requested.len() != requested_skill_ids.len() {
        return Err(AgentError::InvalidRequest(
            "Skill 选择包含重复项".to_owned(),
        ));
    }
    let mut snapshots = Vec::with_capacity(unique_requested.len());
    for skill_id in unique_requested {
        if !assigned_skill_ids.contains(&skill_id) {
            return Err(AgentError::Forbidden("skill_not_assigned"));
        }
        let skill = skills
            .iter()
            .find(|skill| skill.id == skill_id)
            .ok_or(AgentError::NotFound("skill_not_found"))?;
        if skill.organization_id != organization_id || !skill.enabled {
            return Err(AgentError::Forbidden("skill_not_runnable"));
        }
        let mut skill_assets: Vec<SkillAssetSnapshot> = assets
            .iter()
            .filter(|asset| asset.skill_id == skill_id)
            .map(|asset| SkillAssetSnapshot {
                asset_id: asset.id,
                name: asset.name.clone(),
                media_type: asset.media_type.clone(),
                size_bytes: asset.size_bytes,
                sha256: asset.sha256.clone(),
            })
            .collect();
        skill_assets.sort_by_key(|asset| asset.asset_id);
        let checksum = sha256_json(&(&skill.name, &skill.instructions, &skill_assets))?;
        snapshots.push(SkillSnapshot {
            skill_id,
            name: skill.name.clone(),
            instructions: skill.instructions.clone(),
            assets: skill_assets,
            checksum,
        });
    }
    Ok(snapshots)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time_utils::now;

    fn skill(organization_id: Uuid, enabled: bool) -> Skill {
        Skill {
            id: Uuid::new_v4(),
            organization_id,
            name: "summarizer".to_owned(),
            description: "summarize".to_owned(),
            instructions: "Keep it short.".to_owned(),
            enabled,
            created_at: now(),
            updated_at: now(),
        }
    }

    #[test]
    fn unassigned_or_disabled_skills_are_rejected() {
        let organization_id = Uuid::new_v4();
        let selected = skill(organization_id, true);
        assert_eq!(
            resolve_selected_skills(
                organization_id,
                &[selected.id],
                &[],
                std::slice::from_ref(&selected),
                &[]
            ),
            Err(AgentError::Forbidden("skill_not_assigned"))
        );
        let mut disabled = selected.clone();
        disabled.enabled = false;
        assert_eq!(
            resolve_selected_skills(
                organization_id,
                &[selected.id],
                &[selected.id],
                &[disabled],
                &[]
            ),
            Err(AgentError::Forbidden("skill_not_runnable"))
        );
    }

    #[test]
    fn snapshot_records_checksum_and_assets() {
        let organization_id = Uuid::new_v4();
        let selected = skill(organization_id, true);
        let asset = SkillAsset {
            id: Uuid::new_v4(),
            skill_id: selected.id,
            name: "guide.md".to_owned(),
            media_type: "text/markdown".to_owned(),
            size_bytes: 3,
            sha256: "a".repeat(64),
        };
        let snapshots = resolve_selected_skills(
            organization_id,
            &[selected.id],
            &[selected.id],
            &[selected],
            &[asset],
        )
        .unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].assets.len(), 1);
        assert_eq!(snapshots[0].checksum.len(), 64);
    }
}
