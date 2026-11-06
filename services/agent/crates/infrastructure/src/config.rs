//! Agent 服务配置加载。
//!
//! 优先级从低到高：代码默认值 < 根 `config.toml` 的 `[agent]` 子树 < Nacos 应用 TOML
//! < 环境变量。agent 使用仓库根配置文件，因此 Nacos 定位信息与 accounts 共用同一个
//! `[nacos]` 区域；`.env.local` 只提供 Nacos 地址与认证信息。
//!
//! - 环境变量格式为 `APP_AGENT__<分区>__<字段>`，如 `APP_AGENT__SERVER__PORT=8091`；
//! - `DATABASE_URL` 直接映射到 `agent.database.url`，与 accounts 的部署习惯一致；
//! - 含连接串、producer token 与服务 token 清单的字段手写 `Debug`，不会进入日志。

use std::collections::BTreeMap;
use std::env;
use std::fmt;
use std::time::Duration;

use figment::Figment;
use figment::providers::{Env, Format, Toml};
use serde::Deserialize;
use tpl_agent_core::runtime_catalog::BuiltinRuntimeDeclaration;

mod nacos;

/// 默认配置文件路径；与 accounts 共用仓库根文件，可用 `APP_CONFIG_FILE` 覆盖。
const DEFAULT_CONFIG_FILE: &str = "config.toml";

/// Agent 配置加载失败。
#[derive(Debug, thiserror::Error)]
#[error("Agent 配置加载失败：{0}")]
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

/// 根配置文件外壳：Agent 只读取 `[agent]` 子树，accounts 的配置不会进入本服务。
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ConfigFile {
    /// Agent 服务配置。
    agent: Settings,
}

/// Agent 服务完整运行配置。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// HTTP 监听配置。
    pub server: ServerSettings,
    /// 日志配置。
    pub logging: LoggingSettings,
    /// 独立数据库配置。
    pub database: DatabaseSettings,
    /// accounts 身份校验与服务凭据配置。
    pub identity: IdentitySettings,
    /// Blob 存储与上限配置。
    pub storage: StorageSettings,
    /// 外部执行配置。
    pub execution: ExecutionSettings,
    /// tracing outbox 投递配置。
    pub tracing: TracingSettings,
    /// 部署配置的平台管理员 accounts ID；不自动获得组织 scope。
    pub platform_admin_ids: Vec<i64>,
    /// 部署声明的内置 Runtime。
    pub builtin_runtimes: Vec<BuiltinRuntimeDeclaration>,
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
            port: 8091,
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
    /// Agent 专属连接串；未配置时服务启动失败。
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

/// accounts 身份校验与调用服务凭据。
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct IdentitySettings {
    /// `GET /auth/me` 的完整 URL。
    pub user_auth_url: String,
    /// `GET /admin/auth/me` 的完整 URL。
    pub admin_auth_url: String,
    /// 调用 accounts 的超时秒数。
    pub request_timeout_secs: u64,
    /// 调用服务凭据 JSON 数组，每项含 `service_id` 与 `token`；只从环境注入。
    pub service_tokens_json: String,
}

impl Default for IdentitySettings {
    fn default() -> Self {
        Self {
            user_auth_url: "http://127.0.0.1:8080/auth/me".to_owned(),
            admin_auth_url: "http://127.0.0.1:8080/admin/auth/me".to_owned(),
            request_timeout_secs: 3,
            service_tokens_json: "[]".to_owned(),
        }
    }
}

impl fmt::Debug for IdentitySettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IdentitySettings")
            .field("user_auth_url", &self.user_auth_url)
            .field("admin_auth_url", &self.admin_auth_url)
            .field("request_timeout_secs", &self.request_timeout_secs)
            .field("service_tokens_json", &"<已配置，已隐藏>")
            .finish()
    }
}

/// Blob 存储与上传上限。
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StorageSettings {
    /// Blob 根目录；资源、Skill 资产与 Run 产物都放在这里。
    pub root: String,
    /// 单个用户资源或 Skill 资产的最大字节数。
    pub max_resource_bytes: usize,
    /// 单个 Run 产物的最大字节数。
    pub max_artifact_bytes: usize,
}

impl Default for StorageSettings {
    fn default() -> Self {
        Self {
            root: "./data/blobs".to_owned(),
            max_resource_bytes: 10 * 1024 * 1024,
            max_artifact_bytes: 10 * 1024 * 1024,
        }
    }
}

/// 外部执行配置。
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ExecutionSettings {
    /// 单次 A2A 请求超时秒数。
    pub a2a_timeout_secs: u64,
}

impl Default for ExecutionSettings {
    fn default() -> Self {
        Self {
            a2a_timeout_secs: 60,
        }
    }
}

/// tracing outbox 投递配置；`ingest_url` 为空时不启动投递 worker。
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct TracingSettings {
    /// tracing producer ingest 基础 URL，例如 `http://127.0.0.1:8090/api/v1/ingest`。
    pub ingest_url: String,
    /// producer Bearer token；未配置时 worker 不启动，outbox 保持待投递。
    pub producer_token: Option<String>,
    /// 空闲轮询间隔毫秒数。
    pub poll_interval_ms: u64,
    /// 单批最多事件数；tracing 契约上限为 100。
    pub batch_events: usize,
}

impl Default for TracingSettings {
    fn default() -> Self {
        Self {
            ingest_url: String::new(),
            producer_token: None,
            poll_interval_ms: 1000,
            batch_events: 100,
        }
    }
}

impl fmt::Debug for TracingSettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TracingSettings")
            .field("ingest_url", &self.ingest_url)
            .field(
                "producer_token",
                &self.producer_token.as_ref().map(|_| "<已配置，已隐藏>"),
            )
            .field("poll_interval_ms", &self.poll_interval_ms)
            .field("batch_events", &self.batch_events)
            .finish()
    }
}

impl Settings {
    /// 加载配置：读取 `.env.local`，合并本地 TOML、可选 Nacos TOML 与环境变量。
    ///
    /// # Errors
    ///
    /// 配置文件格式错误、已启用的 Nacos 读取失败或环境变量类型不匹配时返回错误。
    pub async fn load() -> Result<Self, ConfigError> {
        let _ = dotenvy::from_filename(".env.local");
        let config_file_path =
            env::var("APP_CONFIG_FILE").unwrap_or_else(|_| DEFAULT_CONFIG_FILE.to_owned());
        let nacos_config_content = nacos::load_config_from_environment(&config_file_path)
            .await
            .map_err(|error| ConfigError(Box::new(error)))?;
        build_figment(&config_file_path, nacos_config_content.as_deref())
            .extract::<ConfigFile>()
            .map(|config_file| config_file.agent)
            .map_err(|error| ConfigError(Box::new(error)))
    }

    /// accounts public 用户身份校验超时。
    pub fn identity_timeout(&self) -> Duration {
        Duration::from_secs(self.identity.request_timeout_secs.max(1))
    }

    /// tracing 空闲轮询间隔。
    pub fn tracing_poll_interval(&self) -> Duration {
        Duration::from_millis(self.tracing.poll_interval_ms.clamp(100, 60_000))
    }
}

/// 按低到高的优先级合并本地文件、Nacos 配置与进程环境变量。
fn build_figment(config_file_path: &str, nacos_config_content: Option<&str>) -> Figment {
    let figment = Figment::new().merge(Toml::file(config_file_path));
    let figment = match nacos_config_content {
        Some(config_content) => figment.merge(Toml::string(config_content)),
        None => figment,
    };
    figment
        .merge(
            Env::prefixed("APP_AGENT__")
                .split("__")
                .map(|key| format!("agent.{key}").into()),
        )
        // 连接串与密钥沿用部署平台常见的名字，避免把秘密写进 TOML。
        .merge(
            Env::raw()
                .only(&["DATABASE_URL"])
                .map(|_| "agent.database.url".to_owned().into()),
        )
        .merge(
            Env::raw()
                .only(&["AGENT_SERVICE_TOKENS_JSON"])
                .map(|_| "agent.identity.service_tokens_json".to_owned().into()),
        )
        .merge(
            Env::raw()
                .only(&["AGENT_TRACING_PRODUCER_TOKEN"])
                .map(|_| "agent.tracing.producer_token".to_owned().into()),
        )
}

impl IdentitySettings {
    /// 解析调用服务凭据清单；空清单表示暂不接受任何服务调用。
    ///
    /// # Errors
    ///
    /// JSON 结构、服务标识或 token 长度不合法时返回配置错误。
    pub fn service_tokens(&self) -> Result<BTreeMap<String, String>, ConfigError> {
        #[derive(Deserialize)]
        struct ServiceTokenEntry {
            service_id: String,
            token: String,
        }
        let entries = serde_json::from_str::<Vec<ServiceTokenEntry>>(&self.service_tokens_json)
            .map_err(|_| ConfigError(Box::new(InvalidServiceTokens)))?;
        let mut tokens = BTreeMap::new();
        for entry in entries {
            if entry.service_id.is_empty()
                || entry.service_id.len() > 128
                || entry.service_id.chars().any(char::is_control)
                || entry.token.len() < 32
                || entry.token.len() > 4096
                || entry.token.chars().any(char::is_control)
            {
                return Err(ConfigError(Box::new(InvalidServiceTokens)));
            }
            if tokens.insert(entry.service_id, entry.token).is_some() {
                return Err(ConfigError(Box::new(InvalidServiceTokens)));
            }
        }
        Ok(tokens)
    }
}

/// 服务凭据清单格式错误；错误信息不回显 token。
#[derive(Debug, thiserror::Error)]
#[error("服务凭据清单必须是 service_id/token 唯一且 token 至少 32 字节的 JSON 数组")]
struct InvalidServiceTokens;

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
            let settings: ConfigFile = build_figment("missing.toml", None).extract()?;
            assert_eq!(settings.agent.server.host, "127.0.0.1");
            assert_eq!(settings.agent.server.port, 8091);
            assert_eq!(settings.agent.logging.format, LogFormat::Text);
            assert!(settings.agent.database.url.is_none());
            assert_eq!(settings.agent.storage.max_resource_bytes, 10 * 1024 * 1024);
            assert!(settings.agent.platform_admin_ids.is_empty());
            assert!(settings.agent.builtin_runtimes.is_empty());
            assert_eq!(settings.agent.identity.service_tokens().unwrap().len(), 0);
            Ok(())
        });
    }

    #[test]
    fn agent_subtree_and_nacos_override_defaults_and_env_wins() {
        Jail::expect_with(|jail| {
            jail.create_file(
                "config.toml",
                "[server]\nport = 8080\n[agent]\nplatform_admin_ids = [1]\n[agent.server]\nport = 8092\n",
            )?;
            let nacos_config =
                "[agent.database]\nmax_connections = 20\n[agent.logging]\nformat = \"json\"\n";
            jail.set_env("APP_AGENT__SERVER__PORT", "8093");
            let settings: ConfigFile =
                build_figment("config.toml", Some(nacos_config)).extract()?;
            assert_eq!(settings.agent.server.port, 8093, "环境变量优先级最高");
            assert_eq!(settings.agent.database.max_connections, 20);
            assert_eq!(settings.agent.logging.format, LogFormat::Json);
            assert_eq!(settings.agent.platform_admin_ids, vec![1]);
            assert_eq!(
                settings.agent.server.host, "127.0.0.1",
                "未写字段保持代码默认值"
            );
            Ok(())
        });
    }

    #[test]
    fn database_url_and_service_tokens_are_recognised_from_plain_env_names() {
        Jail::expect_with(|jail| {
            jail.set_env(
                "DATABASE_URL",
                "postgres://agent:secret@localhost:5433/agent",
            );
            jail.set_env(
                "AGENT_SERVICE_TOKENS_JSON",
                r#"[{"service_id":"web","token":"0123456789abcdef0123456789abcdef"}]"#,
            );
            let settings: ConfigFile = build_figment("missing.toml", None).extract()?;
            assert_eq!(
                settings.agent.database.url.as_deref(),
                Some("postgres://agent:secret@localhost:5433/agent")
            );
            assert_eq!(
                settings
                    .agent
                    .identity
                    .service_tokens()
                    .unwrap()
                    .get("web")
                    .map(String::as_str),
                Some("0123456789abcdef0123456789abcdef")
            );
            Ok(())
        });
    }

    #[test]
    fn debug_output_hides_secrets() {
        let mut settings = Settings::default();
        settings.database.url = Some("postgres://user:secret@localhost/agent".to_owned());
        settings.identity.service_tokens_json =
            r#"[{"service_id":"web","token":"supersecret-token-value-0123456789"}]"#.to_owned();
        settings.tracing.producer_token = Some("producer-secret-token".to_owned());
        let rendered = format!("{settings:?}");
        assert!(!rendered.contains("secret"), "实际输出：{rendered}");
        assert!(!rendered.contains("supersecret"), "实际输出：{rendered}");
    }

    #[test]
    fn short_service_tokens_are_rejected_without_echoing_them() {
        let settings = IdentitySettings {
            service_tokens_json: r#"[{"service_id":"web","token":"short"}]"#.to_owned(),
            ..IdentitySettings::default()
        };
        let error = settings.service_tokens().unwrap_err();
        assert!(!error.to_string().contains("short"));
    }
}
