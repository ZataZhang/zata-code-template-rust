//! 独立业务微服务示例：仅通过 Agent public HTTP 契约完成 Run 创建、查询与取消。
//!
//! 本进程不连接任何数据库，也不引用 Agent 的私有 crate；Agent 地址与服务凭据
//! 由 `AGENT_BASE_URL` / `AGENT_SERVICE_TOKEN` 注入，最终用户身份只经 Cookie 转发。

mod contract;
mod upstream;

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use axum::Json;
use axum::Router;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use reqwest::Method;
use serde_json::json;
use tracing_subscriber::EnvFilter;

use crate::contract::{AgentRun, RunCancelled, RunCreated, SendMessageRequest};
use crate::upstream::{AgentClient, ApiError};

/// 业务服务运行配置。
struct Config {
    /// 监听地址。
    bind: String,
    /// Agent 服务基址。
    agent_base_url: String,
    /// 调用 Agent 的服务凭据。
    agent_service_token: String,
}

impl Config {
    /// 从环境变量读取配置；缺少必填项时拒绝启动。
    fn from_env() -> Result<Self> {
        let bind =
            std::env::var("BUSINESS_SERVICE_BIND").unwrap_or_else(|_| "127.0.0.1:8092".to_owned());
        let agent_base_url = std::env::var("AGENT_BASE_URL").context("缺少 AGENT_BASE_URL")?;
        let agent_service_token =
            std::env::var("AGENT_SERVICE_TOKEN").context("缺少 AGENT_SERVICE_TOKEN")?;
        if agent_base_url.trim().is_empty() {
            bail!("AGENT_BASE_URL 不能为空");
        }
        if agent_service_token.trim().is_empty() {
            bail!("AGENT_SERVICE_TOKEN 不能为空");
        }
        Ok(Self {
            bind,
            agent_base_url,
            agent_service_token,
        })
    }
}

/// 读取并校验路径中的 UUID；只允许十六进制、字母与连字符。
fn path_id(value: &str) -> Result<&str, ApiError> {
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        Ok(value)
    } else {
        Err(ApiError::invalid_request("路径 ID 不合法"))
    }
}

fn header_text<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

/// 发送消息并创建/重放 Canonical Run。
async fn create_run(
    State(agent): State<Arc<AgentClient>>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
    payload: Result<Json<SendMessageRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let session_id = path_id(&session_id)?;
    let Json(body) =
        payload.map_err(|rejection| ApiError::invalid_request(rejection.body_text()))?;
    let upstream = agent
        .request(
            Method::POST,
            &format!("/api/v1/sessions/{session_id}/messages"),
            header_text(&headers, header::COOKIE.as_str()),
            header_text(&headers, "idempotency-key"),
            Some(&body),
        )
        .await?;
    let status = upstream.status;
    let envelope: RunCreated = upstream.parse_success()?;
    Ok((status, Json(envelope)).into_response())
}

/// 查询 Run。
async fn get_run(
    State(agent): State<Arc<AgentClient>>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let run_id = path_id(&run_id)?;
    let upstream = agent
        .request(
            Method::GET,
            &format!("/api/v1/runs/{run_id}"),
            header_text(&headers, header::COOKIE.as_str()),
            None,
            None,
        )
        .await?;
    let status = upstream.status;
    let run: AgentRun = upstream.parse_success()?;
    Ok((status, Json(run)).into_response())
}

/// 协作式取消 Run。
async fn cancel_run(
    State(agent): State<Arc<AgentClient>>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let run_id = path_id(&run_id)?;
    let upstream = agent
        .request(
            Method::POST,
            &format!("/api/v1/runs/{run_id}/cancel"),
            header_text(&headers, header::COOKIE.as_str()),
            None,
            None,
        )
        .await?;
    let status = upstream.status;
    let envelope: RunCancelled = upstream.parse_success()?;
    Ok((status, Json(envelope)).into_response())
}

/// 存活探针。
async fn healthz() -> Json<serde_json::Value> {
    Json(json!({"status": "ok", "service": "agent-business-service"}))
}

fn router(agent: Arc<AgentClient>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route(
            "/api/business/sessions/{session_id}/messages",
            post(create_run),
        )
        .route("/api/business/runs/{run_id}", get(get_run))
        .route("/api/business/runs/{run_id}/cancel", post(cancel_run))
        .with_state(agent)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let config = Config::from_env()?;
    let bind = config.bind.clone();
    let agent = Arc::new(AgentClient::new(
        config.agent_base_url,
        config.agent_service_token,
    )?);
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("无法监听 {bind}"))?;
    tracing::info!(address = %listener.local_addr()?, "业务微服务示例已启动");
    axum::serve(listener, router(agent))
        .await
        .context("业务微服务示例异常退出")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    #[test]
    fn accepts_uuid_like_path_id() {
        assert_eq!(
            path_id("6f5d0f0e-7f4b-4a0e-9d1b-3f5a2c8e9d10").expect("合法 ID"),
            "6f5d0f0e-7f4b-4a0e-9d1b-3f5a2c8e9d10"
        );
    }

    #[test]
    fn rejects_path_traversal() {
        let error = path_id("../../admin").expect_err("路径穿越必须被拒绝");
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert_eq!(error.code, "invalid_request");
    }
}
