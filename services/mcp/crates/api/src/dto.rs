//! REST 管理接口与探针的请求/响应 DTO。
//!
//! 连接响应从不包含凭据明文，只暴露 `credential_configured` 布尔位。

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tpl_mcp_core::{Assignment, AuthType, Connection, ConnectionStatus};
use uuid::Uuid;

/// 错误体。
#[derive(Debug, Serialize)]
pub struct ErrorBody {
    /// 机器可读错误码。
    pub code: String,
    /// 展示用说明。
    pub message: String,
}

/// 错误响应包装。
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    /// 错误详情。
    pub error: ErrorBody,
}

/// 列表响应包装。
#[derive(Debug, Serialize)]
pub struct ListResponse<T> {
    /// 列表项。
    pub items: Vec<T>,
}

/// 存活探针响应。
#[derive(Debug, Serialize)]
pub struct LiveResponse {
    /// 固定为 `ok`。
    pub status: &'static str,
}

/// 基本信息响应。
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    /// 服务标识。
    pub service: &'static str,
    /// 包版本。
    pub version: &'static str,
}

/// 就绪探针响应。
#[derive(Debug, Serialize)]
pub struct ReadinessResponse {
    /// `ready` 或 `unavailable`。
    pub status: &'static str,
    /// 依赖名 → `ok` / `fail`。
    pub checks: BTreeMap<&'static str, &'static str>,
}

/// 连接认证方式。
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthTypeDto {
    /// 匿名连接。
    None,
    /// Bearer 凭据。
    Bearer,
}

impl From<AuthTypeDto> for AuthType {
    fn from(value: AuthTypeDto) -> Self {
        match value {
            AuthTypeDto::None => Self::None,
            AuthTypeDto::Bearer => Self::Bearer,
        }
    }
}

/// 连接状态。
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionStatusDto {
    /// 正常参与授权与工具发现。
    Active,
    /// 停用。
    Disabled,
}

impl From<ConnectionStatusDto> for ConnectionStatus {
    fn from(value: ConnectionStatusDto) -> Self {
        match value {
            ConnectionStatusDto::Active => Self::Active,
            ConnectionStatusDto::Disabled => Self::Disabled,
        }
    }
}

/// 创建连接请求。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateConnectionRequest {
    /// 连接展示名称。
    pub name: String,
    /// 远端 Streamable HTTP endpoint。
    pub endpoint_url: String,
    /// 认证方式。
    pub auth_type: AuthTypeDto,
    /// Bearer 凭据明文；只写入，不返回。
    #[serde(default)]
    pub credential: Option<String>,
}

// 请求含明文凭据，手写 Debug 防止日志或诊断输出泄密。
impl fmt::Debug for CreateConnectionRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CreateConnectionRequest")
            .field("name", &self.name)
            .field("endpoint_url", &self.endpoint_url)
            .field("auth_type", &self.auth_type)
            .field("credential", &self.credential.as_ref().map(|_| "<已隐藏>"))
            .finish()
    }
}

/// 更新连接请求；缺省字段保持原值。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateConnectionRequest {
    /// 新名称。
    #[serde(default)]
    pub name: Option<String>,
    /// 新 endpoint。
    #[serde(default)]
    pub endpoint_url: Option<String>,
    /// 新认证方式。
    #[serde(default)]
    pub auth_type: Option<AuthTypeDto>,
    /// 新状态；`disabled` 即停用连接。
    #[serde(default)]
    pub status: Option<ConnectionStatusDto>,
}

/// 设置或轮换凭据请求。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetCredentialRequest {
    /// 认证方式；`none` 表示清除凭据。
    pub auth_type: AuthTypeDto,
    /// 凭据明文；只写入，不返回。
    #[serde(default)]
    pub credential: Option<String>,
}

// 轮换请求同样不得通过 Debug 暴露新凭据。
impl fmt::Debug for SetCredentialRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SetCredentialRequest")
            .field("auth_type", &self.auth_type)
            .field("credential", &self.credential.as_ref().map(|_| "<已隐藏>"))
            .finish()
    }
}

/// 连接响应；凭据只暴露是否已配置。
#[derive(Debug, Serialize)]
pub struct ConnectionResponse {
    /// 连接 ID。
    pub id: Uuid,
    /// 展示名称。
    pub name: String,
    /// 远端 endpoint。
    pub endpoint_url: String,
    /// 认证方式。
    pub auth_type: &'static str,
    /// 是否已保存凭据。
    pub credential_configured: bool,
    /// 连接状态。
    pub status: &'static str,
    /// 创建者主体。
    pub created_by: String,
    /// 创建时间（RFC 3339，与 OpenAPI `format: date-time` 一致）。
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// 最后更新时间（RFC 3339）。
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

impl From<&Connection> for ConnectionResponse {
    fn from(connection: &Connection) -> Self {
        Self {
            id: connection.id,
            name: connection.name.clone(),
            endpoint_url: connection.endpoint_url.to_string(),
            auth_type: connection.auth_type.as_str(),
            credential_configured: connection.credential_configured(),
            status: connection.status.as_str(),
            created_by: connection.created_by.as_str().to_owned(),
            created_at: connection.created_at,
            updated_at: connection.updated_at,
        }
    }
}

/// 授权主体响应。
#[derive(Debug, Serialize)]
pub struct AssignmentResponse {
    /// `user` 或 `group`。
    pub subject_type: &'static str,
    /// 主体标识。
    pub subject_id: String,
}

impl From<&Assignment> for AssignmentResponse {
    fn from(assignment: &Assignment) -> Self {
        Self {
            subject_type: assignment.subject.subject_type().as_str(),
            subject_id: assignment.subject.subject_id().to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use time::OffsetDateTime;
    use tpl_mcp_core::{
        AssignmentSubject, ConnectionStatus, CredentialCiphertext, GroupId, SubjectId, TenantScope,
    };
    use url::Url;

    use super::*;
    use tpl_mcp_core::{ConsumerId, TenantId};

    fn connection_with_ciphertext() -> Connection {
        Connection {
            id: Uuid::from_u128(7),
            scope: TenantScope {
                consumer_id: ConsumerId::new("consumer-a").expect("测试消费服务有效"),
                tenant_id: TenantId::new("tenant-a").expect("测试租户有效"),
            },
            name: "docs".to_owned(),
            endpoint_url: Url::parse("https://mcp.example.com/mcp").expect("测试地址有效"),
            auth_type: AuthType::Bearer,
            credential: CredentialCiphertext::new(b"ciphertext-bytes".to_vec()),
            status: ConnectionStatus::Active,
            created_by: SubjectId::new("admin-1").expect("测试主体有效"),
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn connection_response_exposes_only_configured_status() {
        let response = ConnectionResponse::from(&connection_with_ciphertext());
        let serialized = serde_json::to_value(&response).expect("响应可序列化");
        let object = serialized.as_object().expect("响应是 JSON 对象");
        assert_eq!(
            object.get("credential_configured"),
            Some(&Value::Bool(true))
        );
        assert!(!object.contains_key("credential"));
        assert!(!object.contains_key("credential_ciphertext"));
        assert!(!serialized.to_string().contains("ciphertext-bytes"));
        assert_eq!(object.get("auth_type"), Some(&json!("bearer")));
        assert_eq!(object.get("status"), Some(&json!("active")));
    }

    #[test]
    fn timestamps_serialize_as_rfc3339_strings() {
        let mut connection = connection_with_ciphertext();
        connection.created_at =
            OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("测试时间戳有效");
        connection.updated_at = connection.created_at;

        let response = ConnectionResponse::from(&connection);
        let serialized = serde_json::to_value(&response).expect("响应可序列化");
        assert_eq!(
            serialized.get("created_at"),
            Some(&json!("2023-11-14T22:13:20Z"))
        );
        assert_eq!(
            serialized.get("updated_at"),
            Some(&json!("2023-11-14T22:13:20Z"))
        );
    }

    #[test]
    fn requests_reject_unknown_fields_and_bad_auth_types() {
        let unknown_field = serde_json::from_value::<CreateConnectionRequest>(json!({
            "name": "docs",
            "endpoint_url": "https://mcp.example.com/mcp",
            "auth_type": "none",
            "unexpected": true,
        }));
        assert!(unknown_field.is_err());

        let bad_auth_type = serde_json::from_value::<CreateConnectionRequest>(json!({
            "name": "docs",
            "endpoint_url": "https://mcp.example.com/mcp",
            "auth_type": "oauth",
        }));
        assert!(bad_auth_type.is_err());

        let valid = serde_json::from_value::<CreateConnectionRequest>(json!({
            "name": "docs",
            "endpoint_url": "https://mcp.example.com/mcp",
            "auth_type": "bearer",
            "credential": "token",
        }))
        .expect("合法请求可解析");
        assert!(matches!(AuthType::from(valid.auth_type), AuthType::Bearer));
        assert_eq!(valid.credential.as_deref(), Some("token"));
    }

    #[test]
    fn credential_request_debug_output_is_redacted() {
        let create_request = CreateConnectionRequest {
            name: "docs".to_owned(),
            endpoint_url: "https://mcp.example.com/mcp".to_owned(),
            auth_type: AuthTypeDto::Bearer,
            credential: Some("create-secret".to_owned()),
        };
        let credential_request = SetCredentialRequest {
            auth_type: AuthTypeDto::Bearer,
            credential: Some("rotation-secret".to_owned()),
        };

        let create_debug = format!("{create_request:?}");
        let rotation_debug = format!("{credential_request:?}");
        assert!(!create_debug.contains("create-secret"));
        assert!(!rotation_debug.contains("rotation-secret"));
        assert!(create_debug.contains("已隐藏"));
        assert!(rotation_debug.contains("已隐藏"));
    }

    #[test]
    fn assignment_response_uses_stable_subject_types() {
        let assignment = Assignment {
            connection_id: Uuid::from_u128(7),
            subject: AssignmentSubject::Group(GroupId::new("team-a").expect("测试分组有效")),
        };
        let response = AssignmentResponse::from(&assignment);
        assert_eq!(response.subject_type, "group");
        assert_eq!(response.subject_id, "team-a");
    }
}
