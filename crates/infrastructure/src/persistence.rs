//! 数据库访问：PostgreSQL 连接池、迁移与就绪探针。

use std::time::Duration;

use async_trait::async_trait;
use sqlx::postgres::{PgPool, PgPoolOptions};
use tpl_core::health::ReadinessProbe;

use crate::config::DatabaseSettings;

mod admin_repository;
mod user_repository;

pub use admin_repository::PostgresAdminRepository;
pub use user_repository::PostgresUserRepository;

/// 创建 PostgreSQL 连接池。
///
/// 连接是**惰性**建立的：数据库暂时不可用时服务照常启动，由 `/ready` 如实报告不可用，
/// 而不是让进程在启动阶段崩溃后被反复重启。
///
/// # Errors
///
/// 连接串无法解析时返回错误。
pub fn connect_lazy(
    database_settings: &DatabaseSettings,
    database_url: &str,
) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(database_settings.max_connections)
        .acquire_timeout(Duration::from_secs(database_settings.acquire_timeout_secs))
        .connect_lazy(database_url)
}

/// 执行仓库根目录 `migrations/` 下尚未应用的迁移。可重复调用：已应用的会被跳过。
///
/// SQL 文件在**编译期**嵌入二进制（`build.rs` 监视该目录），所以部署时不需要带上 `migrations/` 目录，
/// 但构建镜像时 `migrations/` 必须在构建上下文里。
///
/// # Errors
///
/// 数据库不可达、迁移 SQL 出错，或已应用的迁移文件被改动过时返回错误。
pub async fn run_migrations(connection_pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("../../migrations")
        .run(connection_pool)
        .await
}

/// PostgreSQL 就绪探针：执行 `SELECT 1`。
pub struct PostgresReadinessProbe {
    connection_pool: PgPool,
    probe_timeout: Duration,
}

impl PostgresReadinessProbe {
    /// 基于连接池创建探针；`probe_timeout` 是整次探测（含获取连接）的上限。
    pub fn new(connection_pool: PgPool, probe_timeout: Duration) -> Self {
        Self {
            connection_pool,
            probe_timeout,
        }
    }
}

#[async_trait]
impl ReadinessProbe for PostgresReadinessProbe {
    fn dependency_name(&self) -> &'static str {
        "postgres"
    }

    async fn check(&self) -> Result<(), String> {
        let select_one_query =
            sqlx::query_scalar!("SELECT 1 AS \"value!\"").fetch_one(&self.connection_pool);

        match tokio::time::timeout(self.probe_timeout, select_one_query).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(query_error)) => Err(format!("查询失败：{query_error}")),
            Err(_elapsed) => Err(format!("探测超时（{:?}）", self.probe_timeout)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn probe_reports_failure_when_database_is_unreachable() {
        // 1 号端口基本不会有人监听：连接会立刻被拒绝，不需要任何外部服务。
        let database_settings = DatabaseSettings::default();
        let connection_pool = connect_lazy(
            &database_settings,
            "postgres://nobody:nopass@127.0.0.1:1/none",
        )
        .expect("连接串格式正确，惰性建池不应失败");
        let probe = PostgresReadinessProbe::new(connection_pool, Duration::from_secs(2));

        let probe_result = probe.check().await;

        assert!(probe_result.is_err(), "数据库不可达时必须报告失败");
        assert_eq!(probe.dependency_name(), "postgres");
    }

    /// 真实数据库测试：默认跳过，用 `just test real` 显式运行。
    #[tokio::test]
    #[ignore = "需要真实 PostgreSQL，设置 TEST_DATABASE_URL 后用 `just test real` 运行"]
    async fn probe_passes_against_a_real_postgres() {
        let database_url = std::env::var("TEST_DATABASE_URL").expect("请设置 TEST_DATABASE_URL");
        let connection_pool =
            connect_lazy(&DatabaseSettings::default(), &database_url).expect("连接串应当可解析");
        let probe = PostgresReadinessProbe::new(connection_pool, Duration::from_secs(5));

        assert_eq!(probe.check().await, Ok(()));
    }
}
