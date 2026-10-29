//! HTTP listener 与优雅退出处理。

use std::future::Future;

use anyhow::Context;
use axum::Router;
use tokio::net::TcpListener;

use tpl_agent_tracing_infrastructure::persistence::PostgresTraceRepository;

/// 运行 HTTP 服务并在退出信号到达后释放 PostgreSQL 连接。
///
/// # Errors
///
/// HTTP server 监听或运行异常时返回错误。
pub async fn serve<ShutdownFuture>(
    listener: TcpListener,
    router: Router,
    repository: PostgresTraceRepository,
    shutdown_signal: ShutdownFuture,
) -> anyhow::Result<()>
where
    ShutdownFuture: Future<Output = ()> + Send + 'static,
{
    let serve_result = axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal)
        .await
        .context("Agent tracing HTTP 服务异常退出");
    repository.close().await;
    serve_result
}
