//! MCP 服务独立进程入口。

mod composition;

use anyhow::Context;
use tpl_mcp_infrastructure::{config::Settings, logging};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let settings = Settings::load().context("加载 MCP 服务配置失败")?;
    logging::init(&settings.logging);
    composition::run(settings).await
}
