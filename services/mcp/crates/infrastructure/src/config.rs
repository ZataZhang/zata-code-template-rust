//! MCP 服务配置加载。
//!
//! 优先级从低到高：代码默认值 < `services/mcp/config.toml` < 环境变量。
//! 本服务不读取仓库根配置或 Nacos：它拥有独立数据库与凭据，配置随服务目录发布。
//!
//! - 环境变量格式为 `APP_MCP__<分区>__<字段>`，如 `APP_MCP__SERVER__PORT=8092`；
//! - `MCP_DATABASE_URL`、`MCP_CREDENTIAL_KEY_BASE64`、`MCP_CONSUMERS_JSON`
//!   直接映射到对应字段，避免把秘密写进 TOML；
//! - 含连接串、主密钥与消费方凭据的字段手写 `Debug`，不会进入日志。

use std::env;
use std::fmt;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use figment::Figment;
use figment::providers::{Env, Format, Toml};
use serde::Deserialize;

/// 默认配置文件路径，可用 `APP_MCP_CONFIG_FILE` 覆盖。
const DEFAULT_CONFIG_FILE: &str = "services/mcp/config.toml";

/// MCP 配置加载失败。
#[derive(Debug, thiserror::Error)]
#[error("MCP 配置加载失败：{0}")]
pub struct ConfigError(#[source] Box<dyn std::error::Error + Send + Sync>);

/// 日志输出格式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    /// 适合本地阅读的文本。
    #[default]
    Text,
    /// 一行一个 JSON。
    Json,
}

/// MCP 服务完整运行配置。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// HTTP 监听配置。
    pub server: ServerSettings,
    /// 日志配置。
    pub logging: LoggingSettings,
    /// 独立数据库配置。
    pub database: DatabaseSettings,
    /// 服务凭据、上下文签名与出站安全配置。
    pub security: SecuritySettings,
    /// 远端调用限制。
    pub outbound: OutboundSettings,
    /// MCP endpoint 的 HTTP 层限制。
    pub gateway: GatewaySettings,
}

/// HTTP 监听配置。
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerSettings {
    /// 监听地址。
    pub host: String,
    /// 监听端口。
    pub port: u16,
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_owned(),
            port: 8092,
        }
    }
}

/// 日志配置。
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LoggingSettings {
    /// 默认日志级别（`RUST_LOG` 优先）。
    pub level: String,
    /// 输出格式。
    pub format: LogFormat,
}

impl Default for LoggingSettings {
    fn default() -> Self {
        Self {
            level: "info".to_owned(),
            format: LogFormat::Text,
        }
    }
}

/// PostgreSQL 连接配置。
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct DatabaseSettings {
    /// MCP 专属连接串；未配置时服务启动失败。
    pub url: Option<String>,
    /// 连接池最大连接数。
    pub max_connections: u32,
    /// 获取连接与 readiness 探测的超时秒数。
    pub acquire_timeout_secs: u64,
}

impl Default for DatabaseSettings {
    fn default() -> Self {
        Self {
            url: None,
            max_connections: 10,
            acquire_timeout_secs: 3,
        }
    }
}

// 连接串可能带密码，手写 Debug 避免误入日志。
impl fmt::Debug for DatabaseSettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DatabaseSettings")
            .field("url", &self.url.as_ref().map(|_| "<已配置，已隐藏>"))
            .field("max_connections", &self.max_connections)
            .field("acquire_timeout_secs", &self.acquire_timeout_secs)
            .finish()
    }
}

/// 服务凭据、上下文签名与出站安全配置。
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct SecuritySettings {
    /// AES-256-GCM 主密钥（标准 Base64 编码的 32 字节）；只从环境注入。
    pub credential_key_base64: Option<String>,
    /// 消费服务清单 JSON，每项含 `consumer_id`、`token`、`context_signing_secret`；只从环境注入。
    pub consumers_json: String,
    /// 用户上下文签发方。
    pub context_issuer: String,
    /// 用户上下文接收方。
    pub context_audience: String,
    /// 用户上下文最大有效期秒数。
    pub context_max_age_secs: i64,
    /// 校验时间戳时允许的时钟偏差秒数。
    pub context_clock_skew_secs: i64,
    /// 仅开发环境：允许访问 loopback 上的 HTTP 测试 MCP server。
    pub allow_insecure_loopback: bool,
}

impl Default for SecuritySettings {
    fn default() -> Self {
        Self {
            credential_key_base64: None,
            consumers_json: "[]".to_owned(),
            context_issuer: "tpl-consumer-backend".to_owned(),
            context_audience: "tpl-mcp-gateway".to_owned(),
            context_max_age_secs: 300,
            context_clock_skew_secs: 30,
            allow_insecure_loopback: false,
        }
    }
}

impl fmt::Debug for SecuritySettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecuritySettings")
            .field(
                "credential_key_base64",
                &self
                    .credential_key_base64
                    .as_ref()
                    .map(|_| "<已配置，已隐藏>"),
            )
            .field("consumers_json", &"<已配置，已隐藏>")
            .field("context_issuer", &self.context_issuer)
            .field("context_audience", &self.context_audience)
            .field("context_max_age_secs", &self.context_max_age_secs)
            .field("context_clock_skew_secs", &self.context_clock_skew_secs)
            .field("allow_insecure_loopback", &self.allow_insecure_loopback)
            .finish()
    }
}

/// 远端调用限制。
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct OutboundSettings {
    /// TCP 连接超时毫秒数。
    pub connect_timeout_ms: u64,
    /// 单次远端请求总超时毫秒数。
    pub request_timeout_ms: u64,
    /// SSE 事件、JSON HTTP 响应、分页工具定义累计结果与工具结果的字节上限（至少 1024）。
    /// JSON HTTP 响应会在反序列化前受此限制；rmcp 自身另有固定 16 MiB 硬上限。
    pub max_response_bytes: usize,
    /// 单个连接最多暴露的工具数量。
    pub max_tools_per_connection: usize,
}

impl Default for OutboundSettings {
    fn default() -> Self {
        Self {
            connect_timeout_ms: 5_000,
            request_timeout_ms: 15_000,
            max_response_bytes: 1024 * 1024,
            max_tools_per_connection: 256,
        }
    }
}

/// MCP endpoint 的 HTTP 层限制。
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct GatewaySettings {
    /// 允许的 Host 头；空列表表示关闭 Host 校验，只应在受控代理之后使用。
    pub allowed_hosts: Vec<String>,
    /// 单个 MCP 请求体上限（字节）。
    pub max_request_body_bytes: usize,
}

impl Default for GatewaySettings {
    fn default() -> Self {
        Self {
            allowed_hosts: vec![
                "localhost".to_owned(),
                "127.0.0.1".to_owned(),
                "::1".to_owned(),
            ],
            max_request_body_bytes: 1024 * 1024,
        }
    }
}

impl Settings {
    /// 从固定路径与环境变量加载配置。
    ///
    /// # Errors
    ///
    /// 配置文件格式错误或环境变量类型不匹配时返回错误。
    pub fn load() -> Result<Self, ConfigError> {
        let _ = dotenvy::from_filename(".env.local");
        let config_file_path =
            env::var("APP_MCP_CONFIG_FILE").unwrap_or_else(|_| DEFAULT_CONFIG_FILE.to_owned());
        build_figment(&config_file_path)
            .extract::<Self>()
            .map_err(|error| ConfigError(Box::new(error)))
    }

    /// 数据库连接串；未配置时启动失败。
    ///
    /// # Errors
    ///
    /// 缺少连接串或连接串不是 PostgreSQL URL 时返回错误。
    pub fn database_url(&self) -> Result<&str, ConfigError> {
        let url = self
            .database
            .url
            .as_deref()
            .filter(|url| !url.trim().is_empty())
            .ok_or_else(|| ConfigError(Box::new(MissingDatabaseUrl)))?;
        if !url.starts_with("postgres://") && !url.starts_with("postgresql://") {
            return Err(ConfigError(Box::new(InvalidDatabaseUrl)));
        }
        Ok(url)
    }

    /// 解码凭据加密主密钥。
    ///
    /// # Errors
    ///
    /// 未配置、不是合法 Base64 或不是 32 字节时返回错误。
    pub fn credential_key(&self) -> Result<[u8; 32], ConfigError> {
        let encoded = self
            .security
            .credential_key_base64
            .as_deref()
            .filter(|encoded| !encoded.trim().is_empty())
            .ok_or_else(|| ConfigError(Box::new(MissingCredentialKey)))?;
        let decoded = STANDARD
            .decode(encoded)
            .map_err(|_| ConfigError(Box::new(InvalidCredentialKey)))?;
        <[u8; 32]>::try_from(decoded.as_slice())
            .map_err(|_| ConfigError(Box::new(InvalidCredentialKey)))
    }

    /// 数据库获取连接超时。
    pub fn database_acquire_timeout(&self) -> Duration {
        Duration::from_secs(self.database.acquire_timeout_secs.max(1))
    }

    /// TCP 连接超时。
    pub fn outbound_connect_timeout(&self) -> Duration {
        Duration::from_millis(self.outbound.connect_timeout_ms.clamp(100, 60_000))
    }

    /// 单次远端请求总超时。
    pub fn outbound_request_timeout(&self) -> Duration {
        Duration::from_millis(self.outbound.request_timeout_ms.clamp(100, 120_000))
    }
}

/// 按低到高的优先级合并本地文件与进程环境变量。
fn build_figment(config_file_path: &str) -> Figment {
    Figment::new()
        .merge(Toml::file(config_file_path))
        .merge(
            Env::prefixed("APP_MCP__")
                .split("__")
                .map(|key| format!("{key}").into()),
        )
        // 秘密沿用部署平台容易注入的名字，不写进 TOML。
        .merge(
            Env::raw()
                .only(&["MCP_DATABASE_URL"])
                .map(|_| "database.url".to_owned().into()),
        )
        .merge(
            Env::raw()
                .only(&["MCP_CREDENTIAL_KEY_BASE64"])
                .map(|_| "security.credential_key_base64".to_owned().into()),
        )
        .merge(
            Env::raw()
                .only(&["MCP_CONSUMERS_JSON"])
                .map(|_| "security.consumers_json".to_owned().into()),
        )
}

/// 缺少独立的 MCP 数据库连接串。
#[derive(Debug, thiserror::Error)]
#[error("必须通过 MCP_DATABASE_URL 或 [database].url 配置 MCP 专属 PostgreSQL 连接串")]
struct MissingDatabaseUrl;

/// 数据库连接串不是 PostgreSQL URL。
#[derive(Debug, thiserror::Error)]
#[error("MCP_DATABASE_URL 必须是 postgres:// 或 postgresql:// 连接串")]
struct InvalidDatabaseUrl;

/// 缺少凭据加密主密钥。
#[derive(Debug, thiserror::Error)]
#[error("必须通过 MCP_CREDENTIAL_KEY_BASE64 配置凭据加密主密钥")]
struct MissingCredentialKey;

/// 凭据加密主密钥格式无效。
#[derive(Debug, thiserror::Error)]
#[error("凭据加密主密钥必须是 Base64 编码的 32 字节")]
struct InvalidCredentialKey;

#[cfg(test)]
#[allow(
    clippy::result_large_err,
    reason = "Jail::expect_with 的闭包必须返回 figment::Error，类型由 figment 决定"
)]
mod tests {
    use figment::Jail;

    use super::*;

    #[test]
    fn defaults_apply_when_nothing_is_configured() {
        Jail::expect_with(|_jail| {
            let settings: Settings = build_figment("missing.toml").extract()?;
            assert_eq!(settings.server.port, 8092);
            assert_eq!(settings.server.host, "127.0.0.1");
            assert_eq!(settings.logging.format, LogFormat::Text);
            assert!(settings.database.url.is_none());
            assert!(!settings.security.allow_insecure_loopback);
            assert_eq!(settings.outbound.max_tools_per_connection, 256);
            Ok(())
        });
    }

    #[test]
    fn env_wins_over_file_and_secret_env_names_are_recognised() {
        Jail::expect_with(|jail| {
            jail.create_file(
                "config.toml",
                "[server]\nport = 8093\n[security]\nallow_insecure_loopback = false\n",
            )?;
            jail.set_env("APP_MCP__SERVER__PORT", "8094");
            jail.set_env("APP_MCP__SECURITY__ALLOW_INSECURE_LOOPBACK", "true");
            jail.set_env("MCP_DATABASE_URL", "postgres://mcp:test@localhost:5436/mcp");
            jail.set_env(
                "MCP_CREDENTIAL_KEY_BASE64",
                "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=",
            );
            let settings: Settings = build_figment("config.toml").extract()?;
            assert_eq!(settings.server.port, 8094);
            assert!(settings.security.allow_insecure_loopback);
            assert_eq!(
                settings.database_url().expect("测试连接串有效"),
                "postgres://mcp:test@localhost:5436/mcp"
            );
            assert_eq!(
                settings.credential_key().expect("测试密钥有效"),
                *b"0123456789abcdef0123456789abcdef"
            );
            Ok(())
        });
    }

    #[test]
    fn compose_style_allowed_hosts_override_is_parsed() {
        Jail::expect_with(|jail| {
            jail.set_env(
                "APP_MCP__GATEWAY__ALLOWED_HOSTS",
                r#"["mcp-service","localhost","127.0.0.1","::1"]"#,
            );
            let settings: Settings = build_figment("missing.toml").extract()?;
            assert_eq!(
                settings.gateway.allowed_hosts,
                vec!["mcp-service", "localhost", "127.0.0.1", "::1"]
            );
            Ok(())
        });
    }

    #[test]
    fn secret_values_are_redacted_in_debug_output() {
        let settings = Settings {
            database: DatabaseSettings {
                url: Some("postgres://mcp:secret@localhost/mcp".to_owned()),
                ..DatabaseSettings::default()
            },
            security: SecuritySettings {
                consumers_json: r#"[{"consumer_id":"web","token":"secret"}]"#.to_owned(),
                ..SecuritySettings::default()
            },
            ..Settings::default()
        };
        let debug_output = format!("{settings:?}");
        assert!(!debug_output.contains("secret"));
        assert!(debug_output.contains("已隐藏"));
    }
}
