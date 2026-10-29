//! 进程入口（composition root）。
//!
//! 只负责：读配置 → 初始化日志 → 装配依赖 → 启动 HTTP 服务 → 响应退出信号。
//! 业务规则不写在这里。

use anyhow::Context;
use tpl_accounts::{composition, server, shutdown};
use tpl_infrastructure::config::Settings;
use tpl_infrastructure::logging;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let settings = Settings::load().await?;
    logging::init(&settings.logging);

    let app_state = composition::build_app_state(&settings).await?;
    let router = tpl_api::build_router(app_state);

    let listener =
        tokio::net::TcpListener::bind((settings.server.host.as_str(), settings.server.port))
            .await
            .with_context(|| {
                format!(
                    "无法监听 {}:{}（端口被占用？）",
                    settings.server.host, settings.server.port
                )
            })?;
    tracing::info!(address = %listener.local_addr()?, "服务已启动");

    server::serve(listener, router, shutdown::wait_for_signal())
        .await
        .context("HTTP 服务异常退出")?;

    tracing::info!("服务已退出");
    Ok(())
}
