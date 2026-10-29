//! 初始化 tracing 服务的结构化日志。

use tracing_subscriber::EnvFilter;

use crate::config::LoggingSettings;

/// 初始化进程级 `tracing` subscriber。
pub fn init(settings: &LoggingSettings) {
    let log_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(settings.level.clone()));
    let formatter = tracing_subscriber::fmt()
        .with_env_filter(log_filter)
        .with_target(false);
    if settings.json {
        formatter.json().init();
    } else {
        formatter.init();
    }
}
