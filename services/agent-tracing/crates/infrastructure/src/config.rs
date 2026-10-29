//! 独立 tracing 服务配置加载与 producer 凭据摘要化。

use std::env;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use figment::Figment;
use figment::providers::{Env, Format, Toml};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tpl_agent_tracing_core::model::{
    MAX_PRODUCER_TOKEN_BYTES, MIN_PRODUCER_TOKEN_BYTES, ProducerRegistration,
};

const DEFAULT_CONFIG_FILE: &str = "config.toml";
const DEFAULT_ADMIN_AUTH_URL: &str = "http://accounts:8080/admin/auth/me";

/// tracing 配置加载失败。
#[derive(Debug, thiserror::Error)]
#[error("tracing 配置加载失败")]
pub struct ConfigError(#[source] Box<dyn std::error::Error + Send + Sync>);

/// 服务完整运行配置；不实现 `Debug`，其中包含 producer token 与 OTLP 地址。
#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Settings {
    /// HTTP 监听配置。
    pub server: ServerSettings,
    /// 日志配置。
    pub logging: LoggingSettings,
    /// tracing 数据库配置。
    pub database: DatabaseSettings,
    /// accounts 管理员认证配置。
    pub accounts: AccountsSettings,
    /// producer token 配置。
    pub producer: ProducerSettings,
    /// 可选 OTLP/HTTP 出口。
    pub otlp: OtlpSettings,
}

/// HTTP 监听地址。
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct ServerSettings {
    /// 监听主机。
    pub host: String,
    /// HTTP 端口。
    pub port: u16,
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_owned(),
            port: 8090,
        }
    }
}

/// 日志输出配置。
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct LoggingSettings {
    /// 默认日志过滤器。
    pub level: String,
    /// 是否输出 JSON 日志。
    pub json: bool,
}

impl Default for LoggingSettings {
    fn default() -> Self {
        Self {
            level: "info".to_owned(),
            json: false,
        }
    }
}

/// PostgreSQL 连接配置。
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct DatabaseSettings {
    /// 独立 tracing 数据库连接串。
    pub url: Option<String>,
    /// 最大连接数。
    pub max_connections: u32,
    /// 获取连接与 readiness 探针超时秒数。
    pub timeout_secs: u64,
}

impl Default for DatabaseSettings {
    fn default() -> Self {
        Self {
            url: None,
            max_connections: 10,
            timeout_secs: 3,
        }
    }
}

impl fmt::Debug for DatabaseSettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DatabaseSettings")
            .field("url", &self.url.as_ref().map(|_| "<已隐藏>"))
            .field("max_connections", &self.max_connections)
            .field("timeout_secs", &self.timeout_secs)
            .finish()
    }
}

/// accounts-service 的公开管理员 Cookie 校验接口。
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct AccountsSettings {
    /// `GET /admin/auth/me` 的完整 URL。
    pub admin_auth_url: String,
    /// 管理员 Cookie 名称。
    pub admin_cookie_name: String,
    /// 每次认证校验的超时秒数。
    pub request_timeout_secs: u64,
}

impl Default for AccountsSettings {
    fn default() -> Self {
        Self {
            admin_auth_url: DEFAULT_ADMIN_AUTH_URL.to_owned(),
            admin_cookie_name: "admin_session".to_owned(),
            request_timeout_secs: 2,
        }
    }
}

/// producer 凭据 JSON 环境变量配置。
#[derive(Deserialize, Default)]
#[serde(default)]
pub struct ProducerSettings {
    /// JSON 数组，明文 token 只在启动内存中解析，数据库仅保存摘要。
    pub credentials_json: String,
}

impl fmt::Debug for ProducerSettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProducerSettings")
            .field("credentials_json", &"<已隐藏>")
            .finish()
    }
}

/// 一个 producer 在部署配置里的明文凭据；仅构造配置时短暂存在。
#[derive(Deserialize)]
struct ConfiguredProducer {
    /// 来源标识。
    source_id: String,
    /// 项目标识。
    project_id: String,
    /// bearer token。
    token: String,
}

/// 可选 OTLP/HTTP exporter 配置。
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct OtlpSettings {
    /// 最终 OTLP traces URL；空值表示完全关闭 exporter。
    pub traces_endpoint: Option<String>,
    /// OTLP Resource 的 service.name。
    pub service_name: String,
    /// OTLP Resource 的 service.version。
    pub service_version: String,
    /// OTLP Resource 的 deployment.environment。
    pub environment: String,
    /// exporter 请求超时秒数。
    pub timeout_secs: u64,
}

impl fmt::Debug for OtlpSettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OtlpSettings")
            .field(
                "traces_endpoint",
                &self.traces_endpoint.as_ref().map(|_| "<已隐藏>"),
            )
            .field("service_name", &self.service_name)
            .field("service_version", &self.service_version)
            .field("environment", &self.environment)
            .field("timeout_secs", &self.timeout_secs)
            .finish()
    }
}

impl Default for OtlpSettings {
    fn default() -> Self {
        Self {
            traces_endpoint: None,
            service_name: "agent-tracing".to_owned(),
            service_version: env!("CARGO_PKG_VERSION").to_owned(),
            environment: "development".to_owned(),
            timeout_secs: 2,
        }
    }
}

impl Settings {
    /// 从可选 TOML、`.env.local` 与 `APP_*` 环境变量读取配置。
    ///
    /// # Errors
    ///
    /// TOML、环境变量或 producer JSON 格式无法解析时返回错误。
    pub fn load() -> Result<Self, ConfigError> {
        let _ = dotenvy::from_filename(".env.local");
        let config_path = env::var("APP_CONFIG_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(DEFAULT_CONFIG_FILE));
        let mut settings = Figment::new()
            .merge(Toml::file(config_path))
            .merge(Env::prefixed("APP_").split("__"))
            .extract::<Self>()
            .map_err(|configuration_error| ConfigError(Box::new(configuration_error)))?;
        settings.apply_otlp_environment_defaults();
        settings.validate_environment_overrides()?;
        Ok(settings)
    }

    /// 生成数据库登记项，只返回 token 摘要。
    ///
    /// # Errors
    ///
    /// producer 配置不是合法 JSON、来源重复或 source/project/token 超出边界时返回错误。
    pub fn producer_registrations(&self) -> Result<Vec<ProducerRegistration>, ConfigError> {
        let configured_producers =
            serde_json::from_str::<Vec<ConfiguredProducer>>(&self.producer.credentials_json)
                .map_err(|configuration_error| ConfigError(Box::new(configuration_error)))?;
        let mut source_ids = std::collections::BTreeSet::new();
        configured_producers
            .into_iter()
            .map(|configured_producer| {
                let source_id_is_valid = !configured_producer.source_id.trim().is_empty()
                    && configured_producer.source_id.len() <= 128
                    && !configured_producer.source_id.chars().any(char::is_control);
                let project_id_is_valid = !configured_producer.project_id.trim().is_empty()
                    && configured_producer.project_id.len() <= 128
                    && !configured_producer.project_id.chars().any(char::is_control);
                if !source_id_is_valid
                    || !project_id_is_valid
                    || !(MIN_PRODUCER_TOKEN_BYTES..=MAX_PRODUCER_TOKEN_BYTES)
                        .contains(&configured_producer.token.len())
                    || configured_producer.token.chars().any(char::is_control)
                    || !source_ids.insert(configured_producer.source_id.clone())
                {
                    return Err(ConfigError(Box::new(std::io::Error::other(
                        "producer source/project/token 配置无效",
                    ))));
                }
                let token_hash = Sha256::digest(configured_producer.token.as_bytes()).to_vec();
                Ok(ProducerRegistration {
                    source_id: configured_producer.source_id,
                    project_id: configured_producer.project_id,
                    token_hash,
                })
            })
            .collect()
    }

    /// 返回 accounts HTTP 客户端超时。
    pub fn accounts_timeout(&self) -> Duration {
        Duration::from_secs(self.accounts.request_timeout_secs.clamp(1, 10))
    }

    /// 返回数据库连接与探针超时。
    pub fn database_timeout(&self) -> Duration {
        Duration::from_secs(self.database.timeout_secs.clamp(1, 30))
    }

    fn apply_otlp_environment_defaults(&mut self) {
        if self
            .otlp
            .traces_endpoint
            .as_deref()
            .is_some_and(|endpoint| !endpoint.trim().is_empty())
        {
            return;
        }
        if let Ok(traces_endpoint) = env::var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")
            && !traces_endpoint.trim().is_empty()
        {
            self.otlp.traces_endpoint = Some(traces_endpoint);
            return;
        }
        if let Ok(base_endpoint) = env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
            && !base_endpoint.trim().is_empty()
        {
            let normalized_endpoint = base_endpoint.trim_end_matches('/');
            let traces_endpoint = if normalized_endpoint.ends_with("/traces") {
                normalized_endpoint.to_owned()
            } else {
                format!("{normalized_endpoint}/v1/traces")
            };
            self.otlp.traces_endpoint = Some(traces_endpoint);
        }
    }

    fn validate_environment_overrides(&self) -> Result<(), ConfigError> {
        if self.database.url.as_deref().is_none_or(str::is_empty) {
            return Err(ConfigError(Box::new(std::io::Error::other(
                "tracing 独立数据库 URL 未配置",
            ))));
        }
        if self.accounts.admin_auth_url.len() > 2048 || self.accounts.admin_cookie_name.len() > 128
        {
            return Err(ConfigError(Box::new(std::io::Error::other(
                "accounts 管理员认证配置超出长度上限",
            ))));
        }
        Ok(())
    }
}
