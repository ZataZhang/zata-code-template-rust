//! 进程存活与数据库就绪探针。

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Serialize;

use crate::AppState;

#[derive(Serialize)]
pub(crate) struct HealthResponse {
    status: &'static str,
}

pub(crate) async fn live() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

pub(crate) async fn ready(State(app_state): State<AppState>) -> (StatusCode, Json<HealthResponse>) {
    match app_state.repository.check_readiness().await {
        Ok(()) => (StatusCode::OK, Json(HealthResponse { status: "ready" })),
        Err(_storage_error) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(HealthResponse {
                status: "unavailable",
            }),
        ),
    }
}
