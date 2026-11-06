//! Runtime connection 目录：builtin/external 来源与 sandbox/a2a 协议彼此独立。
//!
//! 内置 Runtime 由部署声明并在管理面只读；外部 Runtime 由组织管理员接入 A2A。
//! Agent 在创建时绑定 Runtime，Run 快照冻结绑定，后续配置变化不改写历史 Run。

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use url::Url;
use uuid::Uuid;

use crate::error::AgentError;
use crate::organization::validate_text;

/// Runtime 的来源：部署内置或组织外部接入。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeOrigin {
    /// 部署配置声明，管理面只读。
    Builtin,
    /// 组织管理员登记的外部连接。
    External,
}

impl RuntimeOrigin {
    /// 数据库与 API 使用的稳定字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::External => "external",
        }
    }

    /// 从数据库字符串恢复。
    pub fn from_database(value: &str) -> Option<Self> {
        match value {
            "builtin" => Some(Self::Builtin),
            "external" => Some(Self::External),
            _ => None,
        }
    }
}

/// Runtime 的执行协议。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeAdapterKind {
    /// 受限 Sandbox 平台工具执行器。
    Sandbox,
    /// 外部 A2A 1.0 JSON-RPC。
    A2a,
}

impl RuntimeAdapterKind {
    /// 数据库与 API 使用的稳定字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sandbox => "sandbox",
            Self::A2a => "a2a",
        }
    }

    /// 从数据库字符串恢复。
    pub fn from_database(value: &str) -> Option<Self> {
        match value {
            "sandbox" => Some(Self::Sandbox),
            "a2a" => Some(Self::A2a),
            _ => None,
        }
    }
}

/// Runtime 当前是否可执行。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStatus {
    /// 最近声明或发现成功。
    Ready,
    /// 明确不可用；Run 创建时 fail closed。
    Unavailable,
}

impl RuntimeStatus {
    /// 数据库与 API 使用的稳定字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Unavailable => "unavailable",
        }
    }

    /// 从数据库字符串恢复。
    pub fn from_database(value: &str) -> Option<Self> {
        match value {
            "ready" => Some(Self::Ready),
            "unavailable" => Some(Self::Unavailable),
            _ => None,
        }
    }
}

/// 内置 Sandbox 支持的工具白名单；未实现的工具不允许声明为可用。
pub const SANDBOX_KNOWN_TOOLS: &[&str] = &["text.summarize", "artifact.write", "delegate_task"];

/// 非秘密的 Runtime 运行配置。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeConfig {
    /// 允许调用的平台工具。
    pub allowed_tools: Vec<String>,
    /// Sandbox 单次 Run 最大步骤数。
    pub max_steps: u32,
    /// A2A 期待协议版本，仅用于展示；实际以 Agent Card 为准。
    pub a2a_protocol_version: Option<String>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            allowed_tools: vec!["text.summarize".to_owned(), "artifact.write".to_owned()],
            max_steps: 8,
            a2a_protocol_version: None,
        }
    }
}

impl RuntimeConfig {
    /// 校验配置边界与已知工具。
    ///
    /// # Errors
    ///
    /// 工具重复/未知或步数越界时返回错误。
    pub fn validate(&self, adapter_kind: RuntimeAdapterKind) -> Result<(), AgentError> {
        if self.max_steps == 0 || self.max_steps > 32 {
            return Err(AgentError::InvalidRequest(
                "Runtime max_steps 必须在 1-32 之间".to_owned(),
            ));
        }
        if adapter_kind == RuntimeAdapterKind::Sandbox {
            let mut seen = std::collections::BTreeSet::new();
            for tool in &self.allowed_tools {
                if !SANDBOX_KNOWN_TOOLS.contains(&tool.as_str()) {
                    return Err(AgentError::InvalidRequest(format!(
                        "未知的 Sandbox 工具 `{tool}`"
                    )));
                }
                if !seen.insert(tool) {
                    return Err(AgentError::InvalidRequest(
                        "Sandbox 工具白名单包含重复项".to_owned(),
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Runtime connection 目录项。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeConnection {
    /// Runtime UUID。
    pub id: Uuid,
    /// 外部 Runtime 的所属组织；内置为 `None`。
    pub organization_id: Option<Uuid>,
    /// 稳定名称。
    pub name: String,
    /// 展示名称。
    pub display_name: String,
    /// 来源。
    pub origin: RuntimeOrigin,
    /// 协议。
    pub adapter_kind: RuntimeAdapterKind,
    /// 外部 A2A Agent Card 基础 URL；Sandbox 为空。
    pub endpoint_url: Option<String>,
    /// 外部凭据别名，真实密钥不落库。
    pub credential_alias: Option<String>,
    /// 最近状态。
    pub status: RuntimeStatus,
    /// 低敏状态说明。
    pub status_detail: Option<String>,
    /// 非秘密运行配置。
    pub config: RuntimeConfig,
    /// 创建时间。
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// 最近更新时间。
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// 部署声明的内置 Runtime。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuiltinRuntimeDeclaration {
    /// 稳定名称。
    pub name: String,
    /// 展示名称。
    pub display_name: String,
    /// 首发只允许 Sandbox。
    pub adapter_kind: RuntimeAdapterKind,
    /// 是否启用；禁用的声明仍登记为 unavailable。
    pub enabled: bool,
    /// 工具白名单。
    pub allowed_tools: Vec<String>,
    /// 最大步骤数。
    pub max_steps: u32,
}

impl BuiltinRuntimeDeclaration {
    /// 校验声明边界。
    ///
    /// # Errors
    ///
    /// 名称/展示名不合法、协议不是 Sandbox 或工具配置无效时返回错误。
    pub fn validate(&self) -> Result<(), AgentError> {
        validate_text(&self.name, 1, 128, "内置 Runtime 名称")?;
        validate_text(&self.display_name, 1, 200, "内置 Runtime 展示名称")?;
        if self.adapter_kind != RuntimeAdapterKind::Sandbox {
            return Err(AgentError::InvalidRequest(
                "内置 Runtime 首发只支持 sandbox 协议".to_owned(),
            ));
        }
        RuntimeConfig {
            allowed_tools: self.allowed_tools.clone(),
            max_steps: self.max_steps,
            a2a_protocol_version: None,
        }
        .validate(self.adapter_kind)
    }

    /// 转换为可持久化的目录项；id 与时间由存储层补齐。
    pub fn to_connection(
        &self,
        id: Uuid,
        created_at: OffsetDateTime,
        updated_at: OffsetDateTime,
    ) -> RuntimeConnection {
        RuntimeConnection {
            id,
            organization_id: None,
            name: self.name.clone(),
            display_name: self.display_name.clone(),
            origin: RuntimeOrigin::Builtin,
            adapter_kind: self.adapter_kind,
            endpoint_url: None,
            credential_alias: None,
            status: if self.enabled {
                RuntimeStatus::Ready
            } else {
                RuntimeStatus::Unavailable
            },
            status_detail: (!self.enabled).then(|| "部署声明已停用".to_owned()),
            config: RuntimeConfig {
                allowed_tools: self.allowed_tools.clone(),
                max_steps: self.max_steps,
                a2a_protocol_version: None,
            },
            created_at,
            updated_at,
        }
    }
}

/// 外部 A2A Runtime 的创建输入。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewExternalRuntime {
    /// 组织内唯一名称。
    pub name: String,
    /// 展示名称。
    pub display_name: String,
    /// Agent Card 基础 URL。
    pub endpoint_url: String,
    /// 可选凭据别名。
    pub credential_alias: Option<String>,
}

impl NewExternalRuntime {
    /// 校验输入并规范化端点。
    ///
    /// # Errors
    ///
    /// 名称、URL 或凭据别名不合法时返回错误。
    pub fn validate(&self) -> Result<(), AgentError> {
        validate_text(&self.name, 1, 128, "Runtime 名称")?;
        validate_text(&self.display_name, 1, 200, "Runtime 展示名称")?;
        normalize_endpoint_url(&self.endpoint_url)?;
        if let Some(credential_alias) = &self.credential_alias {
            validate_text(credential_alias, 1, 128, "凭据别名")?;
        }
        Ok(())
    }
}

/// 校验并规范化外部 A2A 端点。
///
/// # Errors
///
/// 不是 HTTP(S)、缺少主机、带用户信息或超长时返回错误。
pub fn normalize_endpoint_url(endpoint_url: &str) -> Result<String, AgentError> {
    if endpoint_url.len() > 2048 {
        return Err(AgentError::InvalidRequest("Runtime URL 过长".to_owned()));
    }
    let parsed = Url::parse(endpoint_url)
        .map_err(|_| AgentError::InvalidRequest("Runtime URL 无效".to_owned()))?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        return Err(AgentError::InvalidRequest(
            "Runtime URL 必须是无用户信息、无片段、带主机的 HTTP(S) 地址".to_owned(),
        ));
    }
    Ok(parsed.to_string().trim_end_matches('/').to_owned())
}

/// 校验 Agent 可以绑定该 Runtime。
///
/// # Errors
///
/// 外部 Runtime 与 Agent 不同组织、协议未登记或 Runtime 不可用时返回错误。
pub fn ensure_bindable(
    organization_id: Uuid,
    runtime: &RuntimeConnection,
) -> Result<(), AgentError> {
    let ownership_is_valid = match runtime.origin {
        RuntimeOrigin::Builtin => runtime.organization_id.is_none(),
        RuntimeOrigin::External => runtime.organization_id == Some(organization_id),
    };
    if !ownership_is_valid {
        return Err(AgentError::InvalidRequest(
            "Runtime 不属于当前组织且不是内置 Runtime".to_owned(),
        ));
    }
    if runtime.status != RuntimeStatus::Ready {
        return Err(AgentError::RuntimeUnavailable);
    }
    Ok(())
}

/// 校验 Runtime 可用且协议受支持。
///
/// # Errors
///
/// 状态不是 ready 时返回 [`AgentError::RuntimeUnavailable`]。
pub fn ensure_runtime_ready(runtime: &RuntimeConnection) -> Result<(), AgentError> {
    if runtime.status == RuntimeStatus::Ready {
        Ok(())
    } else {
        Err(AgentError::RuntimeUnavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_validation_rejects_unsafe_urls() {
        assert_eq!(
            normalize_endpoint_url("https://agent.example.com/a2a/").unwrap(),
            "https://agent.example.com/a2a"
        );
        assert!(normalize_endpoint_url("file:///etc/passwd").is_err());
        assert!(normalize_endpoint_url("https://user:pass@agent.example.com").is_err());
        assert!(normalize_endpoint_url("https://agent.example.com/#frag").is_err());
    }

    #[test]
    fn builtin_declarations_only_accept_known_sandbox_tools() {
        let declaration = BuiltinRuntimeDeclaration {
            name: "sandbox-default".to_owned(),
            display_name: "Sandbox".to_owned(),
            adapter_kind: RuntimeAdapterKind::Sandbox,
            enabled: true,
            allowed_tools: vec!["text.summarize".to_owned()],
            max_steps: 4,
        };
        assert!(declaration.validate().is_ok());

        let mut invalid = declaration.clone();
        invalid.allowed_tools = vec!["shell.exec".to_owned()];
        assert!(invalid.validate().is_err());

        let mut a2a = declaration;
        a2a.adapter_kind = RuntimeAdapterKind::A2a;
        assert!(a2a.validate().is_err());
    }

    #[test]
    fn external_runtime_must_belong_to_the_agent_organization() {
        let organization_id = Uuid::new_v4();
        let runtime = RuntimeConnection {
            id: Uuid::new_v4(),
            organization_id: Some(Uuid::new_v4()),
            name: "remote".to_owned(),
            display_name: "Remote".to_owned(),
            origin: RuntimeOrigin::External,
            adapter_kind: RuntimeAdapterKind::A2a,
            endpoint_url: Some("https://agent.example.com".to_owned()),
            credential_alias: None,
            status: RuntimeStatus::Ready,
            status_detail: None,
            config: RuntimeConfig::default(),
            created_at: crate::time_utils::now(),
            updated_at: crate::time_utils::now(),
        };
        assert!(ensure_bindable(organization_id, &runtime).is_err());
        let builtin = RuntimeConnection {
            organization_id: None,
            origin: RuntimeOrigin::Builtin,
            adapter_kind: RuntimeAdapterKind::Sandbox,
            endpoint_url: None,
            ..runtime
        };
        assert!(ensure_bindable(organization_id, &builtin).is_ok());
    }
}
