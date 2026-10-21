//! 启动 HTTP 服务。`main` 和端到端测试共用这一处，避免测试里的启动方式和生产不一致。

use std::future::Future;
use std::net::SocketAddr;

use axum::Router;
use tokio::net::TcpListener;

/// 在 `listener` 上提供服务，直到 `shutdown` 完成，然后优雅退出。
///
/// 必须用 `into_make_service_with_connect_info` 启动：处理器靠它拿到 TCP 对端地址，
/// 登录/注册的按 IP 限流依赖这个地址。**漏掉它不会报错，只会让按 IP 的限流悄悄失效**，
/// 所以这个函数是唯一的启动入口，并有走真实 socket 的测试守着（`tests/http_end_to_end.rs`）。
///
/// # Errors
///
/// 底层 I/O 错误。
pub async fn serve(
    listener: TcpListener,
    router: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown)
    .await
}
