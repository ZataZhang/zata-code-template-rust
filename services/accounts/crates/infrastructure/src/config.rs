//! 配置加载。
//!
//! 优先级从低到高：代码默认值 < `config.toml` < Nacos TOML < 环境变量。
//!
//! - 非敏感默认值写在 `config.toml`；
//! - Nacos 的 namespace/group/Data ID 定位信息写在项目配置的 `[nacos]`；
//! - 本地 Nacos 地址与认证凭据写在 `.env.local`（启动时自动加载，不提交到 Git）；
//! - 配置了 `NACOS_SERVER_ADDR` 时，启动时读取 Nacos TOML 配置；
//! - 环境变量格式为 `APP_<分区>__<字段>`（双下划线表示层级），如 `APP_SERVER__PORT=9000`；
//! - 数据库与 Redis 连接串用约定俗成的 `DATABASE_URL`、`REDIS_URL`；
//! - 所有业务字段都有默认值；未启用 Nacos 且 env 没填时配置加载也必须成功。

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use figment::Figment;
use figment::providers::{Env, Format, Toml};
use serde::Deserialize;
use tpl_core::admin::AdminPolicy;
use tpl_core::auth::ThrottlePolicy;

mod compose;
mod nacos;

/// 默认配置文件路径，可用 `APP_CONFIG_FILE` 覆盖。
const DEFAULT_CONFIG_FILE: &str = "config.toml";

/// 配置加载失败。
#[derive(Debug, thiserror::Error)]
#[error("配置加载失败：{0}")]
pub struct ConfigError(#[source] Box<dyn std::error::Error + Send + Sync>);

/// 已校验的 Compose 环境变量。该类型不实现 `Debug`，避免意外记录密钥值。
pub struct ComposeEnvironment(BTreeMap<String, String>);

impl ComposeEnvironment {
    /// 消费配置并返回可直接传给子进程的键值对。
    pub fn into_variables(self) -> impl Iterator<Item = (String, String)> {
        self.0.into_iter()
    }
}

/// 应用全部配置。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// HTTP 服务监听配置。
    pub server: ServerSettings,
    /// 日志配置。
    pub logging: LoggingSettings,
    /// 数据库配置。
    pub database: DatabaseSettings,
    /// Redis 配置（会话存储）。
    pub redis: RedisSettings,
    /// 认证与会话配置。
    pub auth: AuthSettings,
    /// 管理后台配置。
    pub admin: AdminSettings,
}

/// HTTP 服务监听配置。
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerSettings {
    /// 监听地址。容器内需要改成 `0.0.0.0`。
    pub host: String,
    /// 监听端口。
    pub port: u16,
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8080,
        }
    }
}

/// 日志输出格式。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    /// 适合本地阅读的文本。
    #[default]
    Text,
    /// 一行一个 JSON，适合日志采集系统。
    Json,
}

/// 日志配置。
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LoggingSettings {
    /// 默认日志级别（`RUST_LOG` 环境变量优先），如 `info`、`debug`、`tpl_api=debug,info`。
    pub level: String,
    /// 输出格式。
    pub format: LogFormat,
}

impl Default for LoggingSettings {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            format: LogFormat::default(),
        }
    }
}

/// 数据库配置。
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct DatabaseSettings {
    /// PostgreSQL 连接串；未配置时不启用数据库相关功能。
    pub url: Option<String>,
    /// 连接池最大连接数。
    pub max_connections: u32,
    /// 获取连接与探测的超时秒数。
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

// 连接串里可能带密码，所以手写 Debug：日志或 panic 信息里不会泄露它。
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

/// Redis 配置。
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct RedisSettings {
    /// Redis 连接串（`redis://:密码@主机:端口/库`）；未配置时不启用会话与登录。
    pub url: Option<String>,
    /// 单次 Redis 操作（含首次建立连接）的超时秒数。
    pub timeout_secs: u64,
}

impl Default for RedisSettings {
    fn default() -> Self {
        Self {
            url: None,
            timeout_secs: 3,
        }
    }
}

// 连接串里可能带密码，同样手写 Debug。
impl fmt::Debug for RedisSettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RedisSettings")
            .field("url", &self.url.as_ref().map(|_| "<已配置，已隐藏>"))
            .field("timeout_secs", &self.timeout_secs)
            .finish()
    }
}

/// 认证与会话配置。
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AuthSettings {
    /// 会话有效期（秒），同时也是 Cookie 的 `Max-Age`。
    pub session_ttl_secs: u64,
    /// 会话 Cookie 的名字。
    pub cookie_name: String,
    /// Cookie 是否带 `Secure` 属性（只经 HTTPS 发送）。本地 HTTP 开发时为 `false`，**生产环境必须为 `true`**。
    pub cookie_secure: bool,
    /// 同一个 IP 在一个窗口内允许的登录失败次数。
    pub login_max_failures_per_ip: u32,
    /// 同一个账号（不论来自哪个 IP）在一个窗口内允许的登录失败次数。
    pub login_max_failures_per_account: u32,
    /// 登录失败计数的窗口（秒）。
    pub login_window_secs: u64,
    /// 同一个 IP 在一个窗口内允许的注册请求次数。
    pub register_max_per_ip: u32,
    /// 注册计数的窗口（秒）。
    pub register_window_secs: u64,
    /// 是否信任 `X-Forwarded-For` 请求头来识别客户端 IP。
    ///
    /// 默认 `false`：只用 TCP 连接的对端地址，客户端无法伪造。**只有**服务确实部署在会覆盖/追加该头的
    /// 反向代理之后（且外部无法绕过代理直接访问服务）时才能设为 `true`，否则攻击者可以随便填这个头来
    /// 绕过按 IP 的限流。开启后取该头里**最右**的地址（由我们自己的代理追加，客户端伪造不了）。
    pub trust_forwarded_for: bool,
}

impl Default for AuthSettings {
    fn default() -> Self {
        let throttle_defaults = ThrottlePolicy::default();
        Self {
            session_ttl_secs: 7 * 24 * 60 * 60,
            cookie_name: "session".to_string(),
            cookie_secure: false,
            login_max_failures_per_ip: throttle_defaults.login_max_failures_per_ip,
            login_max_failures_per_account: throttle_defaults.login_max_failures_per_account,
            login_window_secs: throttle_defaults.login_window.as_secs(),
            register_max_per_ip: throttle_defaults.register_max_per_ip,
            register_window_secs: throttle_defaults.register_window.as_secs(),
            trust_forwarded_for: false,
        }
    }
}

impl AuthSettings {
    /// 转成 core 使用的限流策略。
    pub fn throttle_policy(&self) -> ThrottlePolicy {
        ThrottlePolicy {
            login_max_failures_per_ip: self.login_max_failures_per_ip,
            login_max_failures_per_account: self.login_max_failures_per_account,
            login_window: Duration::from_secs(self.login_window_secs),
            register_max_per_ip: self.register_max_per_ip,
            register_window: Duration::from_secs(self.register_window_secs),
        }
    }
}

/// 管理后台配置。管理员与普通用户完全隔离：独立的账号表、会话 Cookie 与限流计数。
///
/// Cookie 的 `Secure` 属性与「是否信任代理头」沿用 [`AuthSettings`]，不重复配置。
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct AdminSettings {
    /// 引导管理员的用户名。与密码**同时**配置时，启动会创建这个管理员（已存在则什么都不改）。
    pub bootstrap_username: Option<String>,
    /// 引导管理员的初始密码。只能来自环境变量或密钥管理，不要写进 `config.toml`。
    pub bootstrap_password: Option<String>,
    /// 管理员会话有效期（秒）。比普通用户短：管理权限的泄露代价更大。
    pub session_ttl_secs: u64,
    /// 管理员会话 Cookie 的名字：必须和用户会话不同，两者才能在同一浏览器里并存。
    pub cookie_name: String,
    /// 同一个 IP 在一个窗口内允许的管理员登录失败次数。
    pub login_max_failures_per_ip: u32,
    /// 同一个管理员账号在一个窗口内允许的登录失败次数。
    pub login_max_failures_per_account: u32,
    /// 登录失败计数的窗口（秒）。
    pub login_window_secs: u64,
}

impl Default for AdminSettings {
    fn default() -> Self {
        Self {
            bootstrap_username: None,
            bootstrap_password: None,
            session_ttl_secs: 12 * 60 * 60,
            cookie_name: "admin_session".to_string(),
            login_max_failures_per_ip: 10,
            login_max_failures_per_account: 5,
            login_window_secs: 15 * 60,
        }
    }
}

// 初始密码是密钥，手写 Debug：日志或 panic 信息里不会泄露它。
impl fmt::Debug for AdminSettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdminSettings")
            .field("bootstrap_username", &self.bootstrap_username)
            .field(
                "bootstrap_password",
                &self.bootstrap_password.as_ref().map(|_| "<已配置，已隐藏>"),
            )
            .field("session_ttl_secs", &self.session_ttl_secs)
            .field("cookie_name", &self.cookie_name)
            .field("login_max_failures_per_ip", &self.login_max_failures_per_ip)
            .field(
                "login_max_failures_per_account",
                &self.login_max_failures_per_account,
            )
            .field("login_window_secs", &self.login_window_secs)
            .finish()
    }
}

impl AdminSettings {
    /// 转成 core 使用的管理员策略（限流用管理员自己的上限，注册限流用不到）。
    pub fn policy(&self) -> AdminPolicy {
        AdminPolicy {
            session_ttl: Duration::from_secs(self.session_ttl_secs),
            throttle: ThrottlePolicy {
                login_max_failures_per_ip: self.login_max_failures_per_ip,
                login_max_failures_per_account: self.login_max_failures_per_account,
                login_window: Duration::from_secs(self.login_window_secs),
                ..ThrottlePolicy::default()
            },
        }
    }

    /// 引导账号：用户名与密码都配置了才返回；只配其中一个按未配置处理（由调用方记警告）。
    pub fn bootstrap_credentials(&self) -> Option<(&str, &str)> {
        Some((
            self.bootstrap_username.as_deref()?,
            self.bootstrap_password.as_deref()?,
        ))
    }
}

impl Settings {
    /// 加载配置：读取本地引导环境变量，再合并 `config.toml`、可选的 Nacos TOML 与环境变量。
    ///
    /// # Errors
    ///
    /// 配置文件格式错误、字段类型不匹配（如端口不是数字），或已启用的 Nacos 配置源读取失败时返回错误。
    pub async fn load() -> Result<Self, ConfigError> {
        // `.env.local` 不存在是正常情况（比如生产环境直接注入环境变量）。
        let _ = dotenvy::from_filename(".env.local");

        let config_file_path = config_file_path_from_environment();
        let nacos_config_content = nacos::load_config_from_environment(&config_file_path)
            .await
            .map_err(|error| ConfigError(Box::new(error)))?;
        build_figment_with_nacos(&config_file_path, nacos_config_content.as_deref())
            .extract()
            .map_err(|error| ConfigError(Box::new(error)))
    }
}

/// 从独立的 Nacos Compose Data ID 加载并校验容器环境变量。
///
/// 此入口强制要求 Nacos 可用，不会回退到本地配置文件，也不会加载应用 TOML Data ID。
/// Nacos 地址和认证凭据从 `.env.local` 或进程环境读取；返回值只包含白名单中的 Compose 变量。
///
/// # Errors
///
/// Nacos 参数缺失、认证或读取失败，或远端属性格式、键名和值不符合限制时返回错误。
pub async fn load_compose_environment() -> Result<ComposeEnvironment, ConfigError> {
    let _ = dotenvy::from_filename(".env.local");
    let config_file_path = config_file_path_from_environment();
    let properties = nacos::load_compose_config_from_environment(&config_file_path)
        .await
        .map_err(|error| ConfigError(Box::new(error)))?;
    compose::parse_properties(&properties).map_err(|error| ConfigError(Box::new(error)))
}

fn config_file_path_from_environment() -> String {
    std::env::var("APP_CONFIG_FILE").unwrap_or_else(|_| DEFAULT_CONFIG_FILE.to_string())
}

/// 组装配置来源。文件不存在时 `Toml::file` 会被当作空来源，不会报错。
#[cfg(test)]
fn build_figment(config_file_path: &str) -> Figment {
    build_figment_with_nacos(config_file_path, None)
}

/// 按低到高的优先级合并本地文件、Nacos 配置与进程环境变量。
fn build_figment_with_nacos(config_file_path: &str, nacos_config_content: Option<&str>) -> Figment {
    let figment = Figment::new().merge(Toml::file(config_file_path));
    let figment = match nacos_config_content {
        Some(config_content) => figment.merge(Toml::string(config_content)),
        None => figment,
    };
    figment
        .merge(Env::prefixed("APP_").split("__"))
        .merge(
            Env::raw()
                .only(&["DATABASE_URL"])
                .map(|_| "database.url".into()),
        )
        .merge(Env::raw().only(&["REDIS_URL"]).map(|_| "redis.url".into()))
        // 引导管理员的两个变量也提供不带前缀的写法，和 DATABASE_URL 一样方便在部署平台里配置。
        .merge(
            Env::raw()
                .only(&["ADMIN_BOOTSTRAP_USERNAME"])
                .map(|_| "admin.bootstrap_username".into()),
        )
        .merge(
            Env::raw()
                .only(&["ADMIN_BOOTSTRAP_PASSWORD"])
                .map(|_| "admin.bootstrap_password".into()),
        )
}

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
            assert_eq!(settings.server.host, "127.0.0.1");
            assert_eq!(settings.server.port, 8080);
            assert_eq!(settings.logging.format, LogFormat::Text);
            assert!(settings.database.url.is_none());
            assert!(settings.redis.url.is_none());
            assert_eq!(settings.auth.session_ttl_secs, 7 * 24 * 60 * 60);
            assert_eq!(settings.auth.cookie_name, "session");
            assert!(!settings.auth.cookie_secure);
            assert!(
                !settings.auth.trust_forwarded_for,
                "默认不信任 X-Forwarded-For，否则攻击者可以伪造 IP 绕过限流"
            );
            assert_eq!(settings.auth.throttle_policy(), ThrottlePolicy::default());
            assert_eq!(settings.admin.cookie_name, "admin_session");
            assert_ne!(settings.admin.cookie_name, settings.auth.cookie_name);
            assert_eq!(settings.admin.bootstrap_credentials(), None);
            let admin_throttle = settings.admin.policy().throttle;
            let user_throttle = settings.auth.throttle_policy();
            assert!(
                admin_throttle.login_max_failures_per_account
                    < user_throttle.login_max_failures_per_account,
                "管理员的默认限流应当比普通用户更严"
            );
            Ok(())
        });
    }

    #[test]
    fn toml_file_overrides_defaults() {
        Jail::expect_with(|jail| {
            jail.create_file(
                "config.toml",
                "[server]\nport = 9100\n[logging]\nformat = \"json\"\n",
            )?;
            let settings: Settings = build_figment("config.toml").extract()?;
            assert_eq!(settings.server.port, 9100);
            assert_eq!(settings.server.host, "127.0.0.1", "未写的字段保持默认值");
            assert_eq!(settings.logging.format, LogFormat::Json);
            Ok(())
        });
    }

    #[test]
    fn environment_overrides_toml_and_database_url_is_recognised() {
        Jail::expect_with(|jail| {
            jail.create_file("config.toml", "[server]\nport = 9100\n")?;
            jail.set_env("APP_SERVER__PORT", "9200");
            jail.set_env("DATABASE_URL", "postgres://user:secret@localhost:5432/app");
            jail.set_env("REDIS_URL", "redis://:secret@localhost:6379/0");
            jail.set_env("APP_AUTH__COOKIE_SECURE", "true");
            jail.set_env("APP_AUTH__LOGIN_MAX_FAILURES_PER_ACCOUNT", "3");
            jail.set_env("APP_AUTH__TRUST_FORWARDED_FOR", "true");
            let settings: Settings = build_figment("config.toml").extract()?;
            assert_eq!(settings.server.port, 9200);
            assert_eq!(
                settings.database.url.as_deref(),
                Some("postgres://user:secret@localhost:5432/app")
            );
            assert_eq!(
                settings.redis.url.as_deref(),
                Some("redis://:secret@localhost:6379/0")
            );
            assert!(settings.auth.cookie_secure);
            assert!(settings.auth.trust_forwarded_for);
            assert_eq!(
                settings
                    .auth
                    .throttle_policy()
                    .login_max_failures_per_account,
                3
            );
            Ok(())
        });
    }

    #[test]
    fn admin_bootstrap_needs_both_values_and_accepts_both_env_spellings() {
        Jail::expect_with(|jail| {
            jail.set_env("ADMIN_BOOTSTRAP_USERNAME", "root");
            let only_username: Settings = build_figment("missing.toml").extract()?;
            assert_eq!(
                only_username.admin.bootstrap_credentials(),
                None,
                "只配一个不算配置完整"
            );

            jail.set_env("APP_ADMIN__BOOTSTRAP_PASSWORD", "s3cret-pass");
            jail.set_env("APP_ADMIN__LOGIN_MAX_FAILURES_PER_ACCOUNT", "2");
            let both: Settings = build_figment("missing.toml").extract()?;
            assert_eq!(
                both.admin.bootstrap_credentials(),
                Some(("root", "s3cret-pass"))
            );
            assert_eq!(
                both.admin.policy().throttle.login_max_failures_per_account,
                2
            );
            Ok(())
        });
    }

    #[test]
    fn wrongly_typed_value_is_an_error_not_a_silent_default() {
        Jail::expect_with(|jail| {
            jail.set_env("APP_SERVER__PORT", "not-a-number");
            let result = build_figment("missing.toml").extract::<Settings>();
            assert!(result.is_err());
            Ok(())
        });
    }

    #[test]
    fn debug_output_never_contains_connection_string_passwords() {
        let settings = Settings {
            database: DatabaseSettings {
                url: Some("postgres://user:secret@localhost:5432/app".to_string()),
                ..DatabaseSettings::default()
            },
            redis: RedisSettings {
                url: Some("redis://:topsecret@localhost:6379/0".to_string()),
                ..RedisSettings::default()
            },
            ..Settings::default()
        };
        let rendered = format!("{settings:?}");
        assert!(!rendered.contains("secret"), "实际输出：{rendered}");
        assert_eq!(rendered.matches("已隐藏").count(), 2);

        let admin = AdminSettings {
            bootstrap_password: Some("hunter2-hunter2".to_string()),
            ..AdminSettings::default()
        };
        let rendered = format!("{admin:?}");
        assert!(!rendered.contains("hunter2"), "实际输出：{rendered}");
    }
}
