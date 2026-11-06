//! Owner-safe Session 与消息模型。
//!
//! Session 与 tracing thread 一对一：Session UUID 直接作为 `thread_id`。
//! 每条 user 消息创建一个 Canonical Run；assistant 消息挂在成功 Run 上。

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::AgentError;
use crate::organization::validate_text;

/// 新 Session 的默认标题。
pub const DEFAULT_SESSION_TITLE: &str = "新会话";

/// 对话 Session。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatSession {
    /// Session UUID，同时是 tracing thread_id。
    pub id: Uuid,
    /// 所属组织。
    pub organization_id: Uuid,
    /// 所有者 public 用户 ID。
    pub owner_user_id: i64,
    /// 显式 Agent；为空表示 Auto 入口。
    pub agent_id: Option<Uuid>,
    /// 展示标题。
    pub title: String,
    /// 创建时间。
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// 最近活动时间。
    #[serde(with = "time::serde::rfc3339")]
    pub last_activity_at: OffsetDateTime,
}

/// 消息角色。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    /// 用户输入。
    User,
    /// Agent 输出。
    Assistant,
}

impl MessageRole {
    /// 数据库字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }

    /// 从数据库字符串恢复。
    pub fn from_database(value: &str) -> Option<Self> {
        match value {
            "user" => Some(Self::User),
            "assistant" => Some(Self::Assistant),
            _ => None,
        }
    }
}

/// Session 内消息。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    /// 消息 UUID。
    pub id: Uuid,
    /// 所属 Session。
    pub session_id: Uuid,
    /// 对应 Run；子 Run 的隐藏消息也可能带值。
    pub run_id: Option<Uuid>,
    /// 角色。
    pub role: MessageRole,
    /// 正文。
    pub content: String,
    /// 引用的资源 UUID。
    pub resource_ids: Vec<Uuid>,
    /// 创建时间。
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// 校验 Session 标题。
///
/// # Errors
///
/// 标题为空、过长或含控制字符时返回错误。
pub fn validate_title(title: &str) -> Result<(), AgentError> {
    validate_text(title, 1, 200, "Session 标题")
}

/// 校验 session 所有者；非所有者返回不存在，避免泄露存在性。
///
/// # Errors
///
/// 所有者不匹配时返回 `session_not_found`。
pub fn ensure_session_owner(session: &ChatSession, user_id: i64) -> Result<(), AgentError> {
    if session.owner_user_id == user_id {
        Ok(())
    } else {
        Err(AgentError::NotFound("session_not_found"))
    }
}

/// 校验消息正文。
///
/// # Errors
///
/// 空白、超过 100000 字符或含控制字符（换行/制表符除外）时返回错误。
pub fn validate_message_content(content: &str) -> Result<(), AgentError> {
    let character_count = content.chars().count();
    let has_forbidden_control = content
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'));
    if character_count == 0 || character_count > 100_000 || has_forbidden_control {
        return Err(AgentError::InvalidRequest(
            "消息正文必须在 1-100000 字符之间且不能包含控制字符".to_owned(),
        ));
    }
    Ok(())
}

/// 校验资源与 Skill 选择个数。
///
/// # Errors
///
/// 数量超过上限时返回错误。
pub fn validate_reference_count(resource_ids: usize, skill_ids: usize) -> Result<(), AgentError> {
    if resource_ids > 20 || skill_ids > 20 {
        return Err(AgentError::InvalidRequest(
            "单条消息最多引用 20 个资源和 20 个 Skill".to_owned(),
        ));
    }
    Ok(())
}

/// 生成默认 Session 标题。
pub fn default_title(content: &str) -> String {
    let compact = content.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut title = String::new();
    for character in compact.chars().take(60) {
        title.push(character);
    }
    if title.is_empty() {
        DEFAULT_SESSION_TITLE.to_owned()
    } else {
        title
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_isolation_reports_not_found() {
        let session = ChatSession {
            id: Uuid::new_v4(),
            organization_id: Uuid::new_v4(),
            owner_user_id: 7,
            agent_id: None,
            title: "hello".to_owned(),
            created_at: crate::time_utils::now(),
            last_activity_at: crate::time_utils::now(),
        };
        assert!(ensure_session_owner(&session, 7).is_ok());
        assert_eq!(
            ensure_session_owner(&session, 8),
            Err(AgentError::NotFound("session_not_found"))
        );
    }

    #[test]
    fn message_validation_allows_newlines_but_rejects_nul() {
        assert!(validate_message_content("line 1\nline 2").is_ok());
        assert!(validate_message_content("nul\0byte").is_err());
        assert!(validate_message_content("   ").is_ok());
        assert!(validate_message_content(&"x".repeat(100_001)).is_err());
    }

    #[test]
    fn default_title_is_bounded() {
        let title = default_title(&"word ".repeat(100));
        assert!(title.chars().count() <= 60);
    }
}
