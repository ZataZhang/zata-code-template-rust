//! 日志初始化（基于 `tracing`）。

use tracing_subscriber::EnvFilter;

use crate::config::{LogFormat, LoggingSettings};

/// 初始化全局日志。`RUST_LOG` 环境变量优先于配置里的 `level`。
///
/// rmcp 在 `debug` 级会打印完整的 JSON-RPC 请求与结果，其中包含工具参数和
/// 工具返回值；这里固定把 `rmcp` 目标压回 `info`，避免运维调高日志级别时
/// 把业务载荷写进日志。重复调用是安全的：全局订阅者已存在时静默跳过。
pub fn init(logging_settings: &LoggingSettings) {
    let mut env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&logging_settings.level));
    if let Ok(directive) = "rmcp=info".parse() {
        env_filter = env_filter.add_directive(directive);
    }
    let subscriber_builder = tracing_subscriber::fmt().with_env_filter(env_filter);

    let init_result = match logging_settings.format {
        LogFormat::Text => subscriber_builder.try_init(),
        LogFormat::Json => subscriber_builder.json().try_init(),
    };
    // 已经初始化过：保持现有订阅者即可。
    let _ = init_result;
}
