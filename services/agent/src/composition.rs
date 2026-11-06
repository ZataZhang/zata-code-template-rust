//! 进程级装配：配置、持久化、执行器、后台 worker 与 HTTP 生命周期。

use std::sync::Arc;

use anyhow::{Context, anyhow};
use tokio::sync::watch;

use tpl_agent_api::server::{AgentApiState, build_router};
use tpl_agent_core::identity::{
    AdminIdentityValidator, ServiceTokenValidator, UserIdentityValidator,
};
use tpl_agent_core::ports::{
    AuditRepository, BlobStore, CatalogRepository, ConversationRepository, CredentialResolver,
    DirectoryRepository, ReadinessProbe, ResourceRepository, RunExecutor, RunRepository,
    RuntimeProbe,
};
use tpl_agent_core::service::{AgentService, AgentServiceConfig, AgentServiceDependencies};
use tpl_agent_engines::CompositeRunExecutor;
use tpl_agent_engines::a2a::A2aRuntimeProbe;
use tpl_agent_infrastructure::config::Settings;
use tpl_agent_infrastructure::identity::{
    AccountsIdentityClient, EnvironmentCredentialResolver, ServiceTokenRegistry,
};
use tpl_agent_infrastructure::postgres::PostgresAgentRepository;
use tpl_agent_infrastructure::storage::FilesystemBlobStore;
use tpl_agent_infrastructure::tracing_outbox::{TracingIngestClient, TracingOutboxWorker};

use crate::dispatch;

/// 同时执行的 Run 数上限。
const RUN_DISPATCH_CONCURRENCY: usize = 4;
/// tracing 投递超时。
const TRACING_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// 装配并启动 Agent 服务，直到收到停止信号。
///
/// # Errors
///
/// 配置缺失、数据库不可达、迁移失败或端口绑定失败时返回错误。
pub async fn run(settings: Settings) -> anyhow::Result<()> {
    let database_url = settings.database.url.clone().ok_or_else(|| {
        anyhow!("必须配置 Agent 数据库连接串（DATABASE_URL 或 agent.database.url）")
    })?;
    let repository = Arc::new(
        PostgresAgentRepository::connect(
            &database_url,
            settings.database.max_connections,
            std::time::Duration::from_secs(settings.database.acquire_timeout_secs.max(1)),
        )
        .await
        .context("连接 Agent 数据库失败")?,
    );
    repository
        .run_migrations()
        .await
        .context("执行 Agent 数据库迁移失败")?;

    let directory: Arc<dyn DirectoryRepository> = repository.clone();
    let catalog: Arc<dyn CatalogRepository> = repository.clone();
    let conversations: Arc<dyn ConversationRepository> = repository.clone();
    let runs: Arc<dyn RunRepository> = repository.clone();
    let resources: Arc<dyn ResourceRepository> = repository.clone();
    let audit: Arc<dyn AuditRepository> = repository.clone();
    let readiness: Arc<dyn ReadinessProbe> = repository.clone();

    let credential_resolver: Arc<dyn CredentialResolver> = Arc::new(EnvironmentCredentialResolver);
    let executor: Arc<dyn RunExecutor> = Arc::new(
        CompositeRunExecutor::new(Arc::clone(&credential_resolver))
            .context("创建 Runtime 执行器失败")?,
    );
    let runtime_probe: Arc<dyn RuntimeProbe> = Arc::new(
        A2aRuntimeProbe::new(Arc::clone(&credential_resolver))
            .context("创建 A2A Runtime 探针失败")?,
    );

    let identity_client = Arc::new(
        AccountsIdentityClient::new(
            settings.identity.user_auth_url.clone(),
            settings.identity.admin_auth_url.clone(),
            settings.identity_timeout(),
        )
        .context("创建 accounts 身份客户端失败")?,
    );
    let users: Arc<dyn UserIdentityValidator> = identity_client.clone();
    let admins: Arc<dyn AdminIdentityValidator> = identity_client.clone();
    let service_tokens = settings
        .identity
        .service_tokens()
        .context("解析服务凭据清单失败")?;
    let services: Arc<dyn ServiceTokenValidator> =
        Arc::new(ServiceTokenRegistry::new(service_tokens));

    let blobs: Arc<dyn BlobStore> =
        Arc::new(FilesystemBlobStore::new(settings.storage.root.clone()));

    let service = Arc::new(AgentService::new(AgentServiceDependencies {
        directory,
        catalog,
        conversations,
        runs: Arc::clone(&runs),
        resources,
        audit,
        blobs,
        executor,
        runtime_probe,
        users: Arc::clone(&users),
        admins: Arc::clone(&admins),
        config: AgentServiceConfig {
            platform_admin_ids: settings.platform_admin_ids.clone(),
            max_resource_bytes: settings.storage.max_resource_bytes,
            max_artifact_bytes: settings.storage.max_artifact_bytes,
            builtin_runtimes: settings.builtin_runtimes.clone(),
        },
    }));
    service
        .initialize()
        .await
        .context("初始化 Agent 服务失败")?;

    let router = build_router(AgentApiState {
        service: Arc::clone(&service),
        services,
        users,
        admins,
        readiness,
        max_upload_bytes: settings.storage.max_resource_bytes,
    });

    let (shutdown_sender, shutdown_receiver) = watch::channel(false);
    let trace_worker = match settings.tracing.producer_token.clone() {
        Some(producer_token) if !settings.tracing.ingest_url.is_empty() => {
            let ingest_client = TracingIngestClient::new(
                settings.tracing.ingest_url.clone(),
                producer_token,
                TRACING_REQUEST_TIMEOUT,
            )
            .context("创建 tracing 投递客户端失败")?;
            Some(TracingOutboxWorker::new(
                Arc::clone(&runs),
                Arc::new(ingest_client),
                settings.tracing_poll_interval(),
            ))
        }
        _ => {
            tracing::info!("tracing ingest 未配置；事件保留在 outbox 等待恢复");
            None
        }
    };
    let trace_handle =
        trace_worker.map(|worker| tokio::spawn(worker.run(shutdown_receiver.clone())));
    let dispatch_handle = dispatch::spawn(
        Arc::clone(&service),
        Arc::clone(&runs),
        shutdown_receiver,
        RUN_DISPATCH_CONCURRENCY,
    );

    let bind_address = format!("{}:{}", settings.server.host, settings.server.port);
    let listener = tokio::net::TcpListener::bind(&bind_address)
        .await
        .with_context(|| format!("绑定 {bind_address} 失败"))?;
    tracing::info!(address = %bind_address, "Agent 服务已启动");
    let serve_result = axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await;

    let _ = shutdown_sender.send(true);
    if let Some(trace_handle) = trace_handle {
        let _ = trace_handle.await;
    }
    let _ = dispatch_handle.await;
    repository.close().await;
    serve_result.context("Agent HTTP 服务异常退出")
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if tokio::signal::ctrl_c().await.is_err() {
            tracing::warn!("无法监听 Ctrl-C 信号");
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut terminate_signal) => {
                terminate_signal.recv().await;
            }
            Err(signal_error) => {
                tracing::warn!(error = %signal_error, "无法监听 SIGTERM 信号");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
    tracing::info!("收到停止信号，开始优雅退出");
}
