//! 优雅退出：收到 Ctrl+C 或 SIGTERM（容器停止时发送）后，让在途请求处理完再退出。

/// 等待退出信号。
pub async fn wait_for_signal() {
    let ctrl_c_signal = async {
        tokio::signal::ctrl_c().await.expect("无法监听 Ctrl+C 信号");
    };

    #[cfg(unix)]
    let terminate_signal = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("无法监听 SIGTERM 信号")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate_signal = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c_signal => {},
        () = terminate_signal => {},
    }
    tracing::info!("收到退出信号，正在优雅关闭");
}
