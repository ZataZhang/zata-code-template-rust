//! 进程退出信号处理。

/// 等待 Unix SIGTERM 或 Ctrl-C。
pub async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut terminate_signal) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {},
                _ = terminate_signal.recv() => {},
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
