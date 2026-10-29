//! tracing 服务 composition root：创建自己的数据库、HTTP API 与 accounts 客户端。

use std::sync::Arc;

use anyhow::Context;
use axum::Router;
use tpl_agent_tracing_api::{self, AppState};
use tpl_agent_tracing_core::ports::{AdminSessionValidator, OtlpExporter, TraceRepository};
use tpl_agent_tracing_infrastructure::config::Settings;
use tpl_agent_tracing_infrastructure::otlp;
use tpl_agent_tracing_infrastructure::persistence::PostgresTraceRepository;
use tpl_agent_tracing_infrastructure::sessions::AccountsAdminSessionValidator;

/// 构建 API 路由与可在 graceful shutdown 时关闭的 tracing repository。
///
/// # Errors
///
/// 数据库地址、迁移、producer 凭据、管理员认证客户端或 OTLP 配置无效时返回错误。
pub async fn build(settings: &Settings) -> anyhow::Result<(Router, PostgresTraceRepository)> {
    let database_url = settings
        .database
        .url
        .as_deref()
        .context("必须为 tracing 服务设置独立数据库 URL")?;
    let repository = PostgresTraceRepository::connect(
        database_url,
        settings.database.max_connections,
        settings.database_timeout(),
    )
    .await
    .context("连接 tracing 专属 PostgreSQL 失败")?;
    repository
        .run_migrations()
        .await
        .context("tracing 数据库迁移失败")?;
    let producer_registrations = settings
        .producer_registrations()
        .context("producer 凭据配置无效")?;
    repository
        .register_producers(&producer_registrations)
        .await
        .context("登记 producer source 失败")?;
    let admin_sessions = Arc::new(
        AccountsAdminSessionValidator::new(
            settings.accounts.admin_auth_url.clone(),
            settings.accounts.admin_cookie_name.clone(),
            settings.accounts_timeout(),
        )
        .context("accounts 管理员认证客户端配置失败")?,
    );
    let otlp_exporter = otlp::build_exporter(&settings.otlp).context("OTLP exporter 配置失败")?;
    let app_state = AppState {
        repository: Arc::new(repository.clone()) as Arc<dyn TraceRepository>,
        admin_sessions: admin_sessions as Arc<dyn AdminSessionValidator>,
        otlp_exporter: otlp_exporter as Arc<dyn OtlpExporter>,
    };
    Ok((tpl_agent_tracing_api::build_router(app_state), repository))
}
