//! 健康探针：`/live`、`/health`、`/ready`。

use std::collections::BTreeMap;

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;

use crate::dto::{HealthResponse, LiveResponse, ReadinessResponse};
use crate::server::McpApiState;

pub(crate) fn routes() -> Router<McpApiState> {
    Router::new()
        .route("/live", get(live))
        .route("/health", get(health))
        .route("/ready", get(ready))
}

async fn live() -> Json<LiveResponse> {
    Json(LiveResponse { status: "ok" })
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        service: "mcp",
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn ready(State(state): State<McpApiState>) -> (StatusCode, Json<ReadinessResponse>) {
    let mut checks = BTreeMap::new();
    let database_ready = match state.readiness.check_ready().await {
        Ok(()) => "ok",
        Err(readiness_error) => {
            tracing::warn!(error = %readiness_error, "MCP 依赖未就绪");
            "fail"
        }
    };
    checks.insert("database", database_ready);
    let is_ready = checks.values().all(|check| *check == "ok");
    let (status_code, status_label) = if is_ready {
        (StatusCode::OK, "ready")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "unavailable")
    };
    (
        status_code,
        Json(ReadinessResponse {
            status: status_label,
            checks,
        }),
    )
}
