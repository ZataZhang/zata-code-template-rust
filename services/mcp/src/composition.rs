//! 进程级装配：配置、持久化、认证、出站策略与 HTTP 生命周期。

use std::sync::Arc;

use anyhow::Context;
use tpl_mcp_api::server::{McpApiState, build_router};
use tpl_mcp_core::ReadinessProbe;
use tpl_mcp_core::service::{McpService, McpServiceConfig, McpServiceDependencies};
use tpl_mcp_infrastructure::auth::HmacCallerAuthenticator;
use tpl_mcp_infrastructure::config::Settings;
use tpl_mcp_infrastructure::crypto::AesGcmCredentialCipher;
use tpl_mcp_infrastructure::endpoint::DnsEndpointPolicy;
use tpl_mcp_infrastructure::postgres::PostgresMcpRepository;
use tpl_mcp_infrastructure::remote_mcp::RmcpRemoteClient;

/// 装配并启动 MCP 服务，直到收到停止信号。
///
/// # Errors
///
/// 配置缺失、认证材料无效、数据库不可达、迁移失败或端口绑定失败时返回错误。
pub async fn run(settings: Settings) -> anyhow::Result<()> {
    let database_url = settings
        .database_url()
        .context("读取 MCP 数据库配置失败")?
        .to_owned();
    let credential_key = settings
        .credential_key()
        .context("读取凭据加密主密钥失败")?;
    let authenticator = HmacCallerAuthenticator::from_settings(&settings.security)
        .context("解析消费服务认证配置失败")?;

    let repository = Arc::new(
        PostgresMcpRepository::connect(
            &database_url,
            settings.database.max_connections,
            settings.database_acquire_timeout(),
        )
        .await
        .context("连接 MCP 数据库失败")?,
    );
    repository
        .run_migrations()
        .await
        .context("执行 MCP 数据库迁移失败")?;

    let service = Arc::new(McpService::new(McpServiceDependencies {
        repository: repository.clone(),
        cipher: Arc::new(AesGcmCredentialCipher::new(&credential_key)),
        endpoint_policy: Arc::new(DnsEndpointPolicy::new(
            settings.security.allow_insecure_loopback,
        )),
        remote_client: Arc::new(RmcpRemoteClient::new(&settings.outbound)),
        config: McpServiceConfig {
            max_tools_per_connection: settings.outbound.max_tools_per_connection,
        },
    }));
    let readiness: Arc<dyn ReadinessProbe> = repository.clone();

    let router = build_router(McpApiState {
        service,
        authenticator: Arc::new(authenticator),
        readiness,
        gateway_allowed_hosts: settings.gateway.allowed_hosts.clone(),
        mcp_max_request_body_bytes: settings.gateway.max_request_body_bytes,
    });

    let bind_address = format!("{}:{}", settings.server.host, settings.server.port);
    let listener = tokio::net::TcpListener::bind(&bind_address)
        .await
        .with_context(|| format!("绑定 {bind_address} 失败"))?;
    tracing::info!(address = %bind_address, "MCP 服务已启动");
    let serve_result = axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await;

    repository.close().await;
    serve_result.context("MCP HTTP 服务异常退出")
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
