//! 依赖装配：把基础设施里的具体实现接到 core 的用例上。

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use sqlx::PgPool;
use tpl_api::{AppState, SessionCookieSettings};
use tpl_core::admin::{AdminPorts, AdminService, BootstrapOutcome};
use tpl_core::auth::{AuthPolicy, AuthPorts, AuthService};
use tpl_core::health::{HealthService, ReadinessProbe};
use tpl_infrastructure::config::Settings;
use tpl_infrastructure::password::Argon2PasswordHasher;
use tpl_infrastructure::persistence::{
    self, PostgresAdminRepository, PostgresReadinessProbe, PostgresUserRepository,
};
use tpl_infrastructure::redis_attempts::RedisAttemptCounter;
use tpl_infrastructure::redis_session::{
    ADMIN_SESSION_NAMESPACE, RedisConnection, RedisReadinessProbe, RedisSessionStore,
    USER_SESSION_NAMESPACE,
};

/// 根据配置构建应用状态。
///
/// - 配置了 `DATABASE_URL`：创建连接池、**执行数据库迁移**、注册数据库就绪探针；
/// - 配置了 `REDIS_URL`：注册 Redis 就绪探针，并启用会话存储；
/// - 两者都配置了才启用认证与管理后台（含登录/注册限流，计数存 Redis），否则相关路由返回 503；
/// - 管理后台启用且配置了引导账号时，启动会创建第一个管理员（已存在则不改动）（配置加载本身仍然成功）。
///
/// 迁移在启动时执行，所以配置了数据库就要求它启动时可达，否则启动失败（快速失败，由编排平台重启）。
/// 启动之后数据库短暂不可用不会让进程退出：连接池是惰性重连的，`/ready` 如实报告。
///
/// # Errors
///
/// 连接串格式不对、迁移失败、或认证服务初始化失败时返回错误。
pub async fn build_app_state(settings: &Settings) -> anyhow::Result<AppState> {
    let mut readiness_probes: Vec<Arc<dyn ReadinessProbe>> = Vec::new();

    let connection_pool = match settings.database.url.as_deref() {
        Some(database_url) => {
            let connection_pool = persistence::connect_lazy(&settings.database, database_url)
                .context("DATABASE_URL 格式不正确")?;
            persistence::run_migrations(&connection_pool)
                .await
                .context("数据库迁移失败（数据库是否可达？）")?;
            readiness_probes.push(Arc::new(PostgresReadinessProbe::new(
                connection_pool.clone(),
                Duration::from_secs(settings.database.acquire_timeout_secs),
            )));
            Some(connection_pool)
        }
        None => {
            tracing::warn!("未配置 DATABASE_URL：已跳过数据库就绪探针与迁移");
            None
        }
    };

    let redis_connection = match settings.redis.url.as_deref() {
        Some(redis_url) => {
            let redis_connection = Arc::new(
                RedisConnection::new(redis_url, Duration::from_secs(settings.redis.timeout_secs))
                    .context("REDIS_URL 格式不正确")?,
            );
            readiness_probes.push(Arc::new(RedisReadinessProbe::new(redis_connection.clone())));
            Some(redis_connection)
        }
        None => {
            tracing::warn!("未配置 REDIS_URL：已跳过 Redis 就绪探针");
            None
        }
    };

    let (auth_service, admin_service) = match (connection_pool, redis_connection) {
        (Some(connection_pool), Some(redis_connection)) => {
            let (auth_service, admin_service) =
                build_services(settings, connection_pool, redis_connection).await?;
            (Some(auth_service), Some(admin_service))
        }
        _ => {
            tracing::warn!("认证与管理后台未启用：需要同时配置 DATABASE_URL 和 REDIS_URL");
            (None, None)
        }
    };

    warn_if_cookie_is_insecure_on_a_public_address(settings);

    Ok(AppState {
        health_service: Arc::new(HealthService::new(readiness_probes)),
        auth_service,
        session_cookie: SessionCookieSettings {
            name: settings.auth.cookie_name.clone(),
            secure: settings.auth.cookie_secure,
        },
        admin_service,
        admin_cookie: SessionCookieSettings {
            name: settings.admin.cookie_name.clone(),
            secure: settings.auth.cookie_secure,
        },
        trust_forwarded_for: settings.auth.trust_forwarded_for,
    })
}

/// 数据库与 Redis 都就绪时，装配普通用户域与管理员域，并按配置引导第一个管理员。
///
/// 两个域共用用户表、密码哈希器和计数器连接，但会话存储用不同的 Redis 键命名空间，
/// 限流计数用不同的键前缀，所以彼此的令牌与失败次数互不相通。
async fn build_services(
    settings: &Settings,
    connection_pool: PgPool,
    redis_connection: Arc<RedisConnection>,
) -> anyhow::Result<(Arc<AuthService>, Arc<AdminService>)> {
    let user_repository = Arc::new(PostgresUserRepository::new(connection_pool.clone()));
    let password_hasher = Arc::new(Argon2PasswordHasher);
    let attempt_counter = Arc::new(RedisAttemptCounter::new(redis_connection.clone()));

    let auth_service = Arc::new(
        AuthService::new(
            AuthPorts {
                user_repository: user_repository.clone(),
                session_store: Arc::new(RedisSessionStore::new(
                    redis_connection.clone(),
                    USER_SESSION_NAMESPACE,
                )),
                password_hasher: password_hasher.clone(),
                attempt_counter: attempt_counter.clone(),
            },
            AuthPolicy {
                session_ttl: Duration::from_secs(settings.auth.session_ttl_secs),
                throttle: settings.auth.throttle_policy(),
            },
        )
        .await
        .context("认证服务初始化失败")?,
    );

    let admin_service = Arc::new(
        AdminService::new(
            AdminPorts {
                admin_repository: Arc::new(PostgresAdminRepository::new(connection_pool)),
                user_repository,
                session_store: Arc::new(RedisSessionStore::new(
                    redis_connection,
                    ADMIN_SESSION_NAMESPACE,
                )),
                password_hasher,
                attempt_counter,
            },
            settings.admin.policy(),
        )
        .await
        .context("管理员服务初始化失败")?,
    );
    bootstrap_admin(settings, &admin_service).await?;
    Ok((auth_service, admin_service))
}

/// 按配置引导第一个管理员：账号已存在则不改动（绝不覆盖密码）。
///
/// 凭据不合规（用户名格式、密码太短）直接让启动失败：悄悄跳过会让人以为管理员已经建好却登不上。
async fn bootstrap_admin(settings: &Settings, admin_service: &AdminService) -> anyhow::Result<()> {
    let Some((username, password)) = settings.admin.bootstrap_credentials() else {
        if settings.admin.bootstrap_username.is_some()
            || settings.admin.bootstrap_password.is_some()
        {
            tracing::warn!("引导管理员需要同时配置用户名和密码，只配了其中一个，已忽略");
        }
        return Ok(());
    };
    match admin_service
        .bootstrap_admin(username, password)
        .await
        .context("引导管理员失败（用户名或密码不合规？）")?
    {
        BootstrapOutcome::Created(admin) => {
            tracing::info!(username = %admin.username, "已创建引导管理员");
        }
        BootstrapOutcome::AlreadyExists => {
            tracing::info!("引导管理员已存在，未做任何修改");
        }
    }
    Ok(())
}

/// 监听的不是本机回环地址、却没开 `Secure` Cookie：会话令牌可能经明文 HTTP 传输，值得提醒。
fn warn_if_cookie_is_insecure_on_a_public_address(settings: &Settings) {
    let listens_on_loopback_only = matches!(
        settings.server.host.as_str(),
        "127.0.0.1" | "localhost" | "::1"
    );
    if !settings.auth.cookie_secure && !listens_on_loopback_only {
        tracing::warn!(
            host = %settings.server.host,
            "会话 Cookie 没有 Secure 属性，而服务监听的不是本机地址；生产环境请设置 APP_AUTH__COOKIE_SECURE=true 并使用 HTTPS"
        );
    }
}
