//! Agent 服务独立进程入口。

mod composition;
mod dispatch;

use anyhow::Context;
use tpl_agent_infrastructure::{config::Settings, logging};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let settings = Settings::load().await.context("加载 Agent 服务配置失败")?;
    logging::init(&settings.logging);
    composition::run(settings).await
}
