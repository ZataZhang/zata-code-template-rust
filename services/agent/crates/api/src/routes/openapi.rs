//! OpenAPI 文档路由。

use axum::Router;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;

use crate::server::AgentApiState;

pub(crate) fn routes() -> Router<AgentApiState> {
    Router::new().route("/openapi.yaml", get(openapi_yaml))
}

async fn openapi_yaml() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/yaml; charset=utf-8")],
        include_str!("../../../../openapi.yaml"),
    )
}
