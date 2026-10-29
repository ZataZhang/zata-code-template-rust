//! Agent tracing 服务的进程入口。

use anyhow::Context;
use tpl_agent_tracing::composition;
use tpl_agent_tracing_infrastructure::config::Settings;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let settings = Settings::load()?;
    tpl_agent_tracing_infrastructure::logging::init(&settings.logging);
    let (router, connection_pool) = composition::build(&settings).await?;
    let listener =
        tokio::net::TcpListener::bind((settings.server.host.as_str(), settings.server.port))
            .await
            .with_context(|| {
                format!("无法监听 {}:{}", settings.server.host, settings.server.port)
            })?;
    tracing::info!(address = %listener.local_addr()?, "Agent tracing 服务已启动");
    tpl_agent_tracing::server::serve(
        listener,
        router,
        connection_pool,
        tpl_agent_tracing::shutdown::wait_for_signal(),
    )
    .await?;
    Ok(())
}
