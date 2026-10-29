//! 健康探针的 HTTP 行为测试：经过真实的路由与中间件，只把依赖替换成可控的探针。

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use tpl_api::{AppState, SessionCookieSettings, build_router};
use tpl_core::health::{HealthService, ReadinessProbe};

struct FixedProbe {
    dependency_name: &'static str,
    fixed_result: Result<(), String>,
}

#[async_trait]
impl ReadinessProbe for FixedProbe {
    fn dependency_name(&self) -> &'static str {
        self.dependency_name
    }

    async fn check(&self) -> Result<(), String> {
        self.fixed_result.clone()
    }
}

fn router_with_probes(probes: Vec<(&'static str, Result<(), String>)>) -> axum::Router {
    let readiness_probes = probes
        .into_iter()
        .map(|(dependency_name, fixed_result)| {
            Arc::new(FixedProbe {
                dependency_name,
                fixed_result,
            }) as Arc<dyn ReadinessProbe>
        })
        .collect();
    build_router(AppState {
        health_service: Arc::new(HealthService::new(readiness_probes)),
        auth_service: None,
        session_cookie: SessionCookieSettings {
            name: "session".to_string(),
            secure: false,
        },
        admin_service: None,
        admin_cookie: SessionCookieSettings {
            name: "admin_session".to_string(),
            secure: false,
        },
        trust_forwarded_for: false,
    })
}

async fn get_json(router: axum::Router, uri: &str) -> (StatusCode, Value) {
    let request = Request::builder().uri(uri).body(Body::empty()).unwrap();
    let response = router.oneshot(request).await.unwrap();
    let status_code = response.status();
    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status_code, serde_json::from_slice(&body_bytes).unwrap())
}

#[tokio::test]
async fn live_is_ok_even_when_dependencies_are_down() {
    let router = router_with_probes(vec![("postgres", Err("连接被拒绝".to_string()))]);
    let (status_code, body) = get_json(router, "/live").await;
    assert_eq!(status_code, StatusCode::OK);
    assert_eq!(body, json!({"status": "ok"}));
}

#[tokio::test]
async fn health_reports_the_crate_version() {
    let (status_code, body) = get_json(router_with_probes(vec![]), "/health").await;
    assert_eq!(status_code, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn ready_is_200_when_every_dependency_is_up() {
    let router = router_with_probes(vec![("postgres", Ok(()))]);
    let (status_code, body) = get_json(router, "/ready").await;
    assert_eq!(status_code, StatusCode::OK);
    assert_eq!(
        body,
        json!({"status": "ready", "checks": {"postgres": "ok"}})
    );
}

#[tokio::test]
async fn ready_is_503_and_hides_the_failure_reason_when_a_dependency_is_down() {
    let router = router_with_probes(vec![
        ("postgres", Ok(())),
        ("redis", Err("password=hunter2 refused".to_string())),
    ]);
    let (status_code, body) = get_json(router, "/ready").await;

    assert_eq!(status_code, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        body,
        json!({"status": "unavailable", "checks": {"postgres": "ok", "redis": "fail"}})
    );
    assert!(
        !body.to_string().contains("hunter2"),
        "失败原因只能进服务端日志，不能出现在响应里"
    );
}

#[tokio::test]
async fn unknown_path_is_404() {
    let request = Request::builder().uri("/nope").body(Body::empty()).unwrap();
    let response = router_with_probes(vec![]).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
