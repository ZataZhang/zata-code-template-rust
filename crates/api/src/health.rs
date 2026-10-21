//! 健康探针路由：`/live`、`/health`、`/ready`。

use std::collections::BTreeMap;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::AppState;

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/live", get(live))
        .route("/health", get(health))
        .route("/ready", get(ready))
}

#[derive(Serialize)]
struct LiveResponse {
    status: &'static str,
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
}

#[derive(Serialize)]
struct ReadinessResponse {
    status: &'static str,
    /// 依赖名 → `"ok"` / `"fail"`。失败原因只写服务端日志，不对外暴露。
    checks: BTreeMap<&'static str, &'static str>,
}

/// 存活探针：进程能响应就算活着，不检查任何依赖。
async fn live() -> Json<LiveResponse> {
    Json(LiveResponse { status: "ok" })
}

/// 基本健康信息。
async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

/// 就绪探针：依赖都可用返回 200，否则 503，让负载均衡暂时不要把流量打过来。
async fn ready(State(app_state): State<AppState>) -> (StatusCode, Json<ReadinessResponse>) {
    let readiness_report = app_state.health_service.check_readiness().await;

    let mut dependency_checks = BTreeMap::new();
    for outcome in &readiness_report.outcomes {
        if let Some(failure_reason) = &outcome.failure_reason {
            tracing::warn!(
                dependency = outcome.dependency_name,
                reason = %failure_reason,
                "依赖未就绪"
            );
        }
        let check_label = if outcome.failure_reason.is_none() {
            "ok"
        } else {
            "fail"
        };
        dependency_checks.insert(outcome.dependency_name, check_label);
    }

    let (status_code, status_label) = if readiness_report.is_ready() {
        (StatusCode::OK, "ready")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "unavailable")
    };
    (
        status_code,
        Json(ReadinessResponse {
            status: status_label,
            checks: dependency_checks,
        }),
    )
}
