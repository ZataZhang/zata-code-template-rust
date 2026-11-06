//! 装配生效提示词与 Skill 运行资产。
//!
//! Run 快照冻结了所选 Skill 的指令与资产摘要；本模块把二者与 Agent 提示词
//! 拼成一次性只读上下文。文本资产内联，二进制资产只保留低敏引用，避免把
//! 任意二进制写进提示词或事件。超过边界时明确失败，不截断交付。

use tpl_agent_core::error::AgentError;
use tpl_agent_core::hash::sha256_hex;
use tpl_agent_core::ports::SkillAssetContent;
use tpl_agent_core::skill_access::SkillSnapshot;

/// 单个内联资产正文上限。
pub const MAX_INLINE_ASSET_BYTES: usize = 64 * 1024;
/// 全部内联资产正文上限。
pub const MAX_INLINE_ASSETS_BYTES: usize = 256 * 1024;
/// 生效提示词上限。
pub const MAX_EFFECTIVE_PROMPT_BYTES: usize = 512 * 1024;

/// 已装配的 Skill 上下文。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillContext {
    /// 包含 Skill 指令与资产引用的生效提示词。
    pub effective_prompt: String,
    /// 本次 Run 选择的 Skill 名称，按 Skill ID 排序。
    pub skill_names: Vec<String>,
    /// 内联进上下文的文本资产。
    pub text_assets: Vec<TextAsset>,
}

/// 内联文本资产。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextAsset {
    /// 所属 Skill 名称。
    pub skill_name: String,
    /// 资产名。
    pub name: String,
    /// MIME 类型。
    pub media_type: String,
    /// 正文。
    pub text: String,
}

/// 判断媒体类型是否可按 UTF-8 文本内联。
pub fn is_text_media_type(media_type: &str) -> bool {
    media_type.starts_with("text/") || media_type == "application/json"
}

/// 装配本次 Run 的生效提示词。
///
/// # Errors
///
/// Skill 与资产快照不一致、资产摘要不符、内联资产或提示词超限时返回错误。
pub fn assemble_skill_context(
    agent_prompt: &str,
    skills: &[SkillSnapshot],
    assets: &[SkillAssetContent],
) -> Result<SkillContext, AgentError> {
    let mut effective_prompt = agent_prompt.trim_end().to_owned();
    let mut skill_names = Vec::with_capacity(skills.len());
    let mut text_assets = Vec::new();
    let mut inline_bytes = 0usize;

    for skill in skills {
        skill_names.push(skill.name.clone());
        effective_prompt.push_str("\n\n## Skill: ");
        effective_prompt.push_str(&skill.name);
        effective_prompt.push('\n');
        effective_prompt.push_str(skill.instructions.trim_end());

        for asset in &skill.assets {
            let content = assets
                .iter()
                .find(|candidate| {
                    candidate.skill_id == skill.skill_id && candidate.name == asset.name
                })
                .ok_or(AgentError::StorageUnavailable)?;
            if content.bytes.len() != usize::try_from(asset.size_bytes).unwrap_or(usize::MAX)
                || sha256_hex(&content.bytes) != asset.sha256
            {
                return Err(AgentError::StorageUnavailable);
            }
            effective_prompt.push_str("\n\n### 运行资产: ");
            effective_prompt.push_str(&asset.name);
            effective_prompt.push_str(" (");
            effective_prompt.push_str(&asset.media_type);
            effective_prompt.push_str(", ");
            effective_prompt.push_str(&asset.size_bytes.to_string());
            effective_prompt.push_str(" 字节, sha256 ");
            effective_prompt.push_str(&asset.sha256);
            effective_prompt.push(')');

            if !is_text_media_type(&asset.media_type) {
                continue;
            }
            if content.bytes.len() > MAX_INLINE_ASSET_BYTES {
                return Err(AgentError::Execution("skill_context_too_large"));
            }
            inline_bytes = inline_bytes.saturating_add(content.bytes.len());
            if inline_bytes > MAX_INLINE_ASSETS_BYTES {
                return Err(AgentError::Execution("skill_context_too_large"));
            }
            let text = std::str::from_utf8(&content.bytes)
                .map_err(|_| AgentError::Execution("skill_asset_invalid"))?;
            effective_prompt.push_str("\n```text\n");
            effective_prompt.push_str(text);
            effective_prompt.push_str("\n```");
            text_assets.push(TextAsset {
                skill_name: skill.name.clone(),
                name: asset.name.clone(),
                media_type: asset.media_type.clone(),
                text: text.to_owned(),
            });
        }
    }

    if effective_prompt.len() > MAX_EFFECTIVE_PROMPT_BYTES {
        return Err(AgentError::Execution("skill_context_too_large"));
    }

    Ok(SkillContext {
        effective_prompt,
        skill_names,
        text_assets,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpl_agent_core::skill_access::SkillAssetSnapshot;

    fn skill(asset: &SkillAssetSnapshot) -> SkillSnapshot {
        SkillSnapshot {
            skill_id: uuid::Uuid::nil(),
            name: "summarizer".to_owned(),
            instructions: "Keep it short.".to_owned(),
            assets: vec![asset.clone()],
            checksum: "a".repeat(64),
        }
    }

    fn asset(content: &[u8]) -> (SkillAssetSnapshot, SkillAssetContent) {
        let sha256 = sha256_hex(content);
        (
            SkillAssetSnapshot {
                asset_id: uuid::Uuid::nil(),
                name: "guide.md".to_owned(),
                media_type: "text/markdown".to_owned(),
                size_bytes: i64::try_from(content.len()).unwrap(),
                sha256: sha256.clone(),
            },
            SkillAssetContent {
                skill_id: uuid::Uuid::nil(),
                name: "guide.md".to_owned(),
                media_type: "text/markdown".to_owned(),
                bytes: content.to_vec(),
            },
        )
    }

    #[test]
    fn instructions_and_text_assets_are_assembled() {
        let (snapshot, content) = asset(b"Include the total.");
        let context =
            assemble_skill_context("You are helpful.", &[skill(&snapshot)], &[content]).unwrap();
        assert!(context.effective_prompt.contains("Keep it short."));
        assert!(context.effective_prompt.contains("Include the total."));
        assert_eq!(context.text_assets.len(), 1);
        assert_eq!(context.skill_names, vec!["summarizer"]);
    }

    #[test]
    fn checksum_mismatch_is_rejected() {
        let (snapshot, mut content) = asset(b"original");
        content.bytes = b"tampered".to_vec();
        assert_eq!(
            assemble_skill_context("p", &[skill(&snapshot)], &[content]),
            Err(AgentError::StorageUnavailable)
        );
    }
}
