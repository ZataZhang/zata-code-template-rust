//! 管理操作与授权拒绝的低敏审计记录。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

/// 审计结果类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    /// 请求被允许。
    Allowed,
    /// 请求被拒绝。
    Denied,
    /// 操作成功。
    Succeeded,
    /// 操作失败。
    Failed,
}

impl AuditOutcome {
    /// 数据库字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::Denied => "denied",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }

    /// 从数据库字符串恢复。
    pub fn from_database(value: &str) -> Option<Self> {
        match value {
            "allowed" => Some(Self::Allowed),
            "denied" => Some(Self::Denied),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// 待写入的审计记录；detail 只放低敏关联信息。
#[derive(Clone, Debug)]
pub struct NewAuditRecord {
    /// 发生时间。
    pub occurred_at: OffsetDateTime,
    /// 调用服务标识。
    pub service_id: String,
    /// 管理员 ID。
    pub admin_user_id: Option<i64>,
    /// 用户 ID。
    pub user_id: Option<i64>,
    /// 组织 ID。
    pub organization_id: Option<Uuid>,
    /// 动作名。
    pub action: String,
    /// 目标标识。
    pub target_id: Option<String>,
    /// 结果。
    pub outcome: AuditOutcome,
    /// 低敏附加信息。
    pub detail: Value,
}

/// 已持久化的审计记录。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditRecord {
    /// 自增 ID。
    pub id: i64,
    /// 发生时间。
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    /// 调用服务标识。
    pub service_id: String,
    /// 管理员 ID。
    pub admin_user_id: Option<i64>,
    /// 用户 ID。
    pub user_id: Option<i64>,
    /// 组织 ID。
    pub organization_id: Option<Uuid>,
    /// 动作名。
    pub action: String,
    /// 目标标识。
    pub target_id: Option<String>,
    /// 结果。
    pub outcome: AuditOutcome,
    /// 低敏附加信息。
    pub detail: Value,
}
