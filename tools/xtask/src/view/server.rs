//! 查看器的 Axum 服务与 HTTP 路由。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Path as RoutePath, Query, Request, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, HeaderValue};
use axum::http::{Response, StatusCode};
use axum::middleware::{self, Next};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::time::sleep;

use super::workspace::{self, Payload};

const WRITE_BODY_LIMIT: usize = 64 * 1024;

#[derive(Clone)]
struct AppState {
    repository_root: Arc<PathBuf>,
    last_request: Arc<Mutex<Instant>>,
}

impl AppState {
    fn new(repository_root: PathBuf) -> Self {
        Self {
            repository_root: Arc::new(repository_root),
            last_request: Arc::new(Mutex::new(Instant::now())),
        }
    }

    fn touch(&self) {
        if let Ok(mut last_request) = self.last_request.lock() {
            *last_request = Instant::now();
        }
    }
}

/// 运行本机查看器服务，并在退出时清理本实例登记。
pub(super) async fn run(repository_root: PathBuf, port: u16, idle_timeout_seconds: f64) -> i32 {
    let listener = match TcpListener::bind(("127.0.0.1", port)).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("查看器无法绑定 127.0.0.1:{port}：{error}");
            return 1;
        }
    };
    let state = AppState::new(repository_root.clone());
    let application = build_router(state.clone());
    let (shutdown_sender, shutdown_receiver) = oneshot::channel::<()>();
    let mut server_task = tokio::spawn(async move {
        axum::serve(listener, application)
            .with_graceful_shutdown(async move {
                let _ = shutdown_receiver.await;
            })
            .await
    });

    eprintln!(
        "查看器服务已监听 http://127.0.0.1:{port}/（仓库 {}，空闲时限 {} 秒）",
        repository_root
            .file_name()
            .unwrap_or_default()
            .to_string_lossy(),
        if idle_timeout_seconds > 0.0 {
            idle_timeout_seconds.round() as u64
        } else {
            0
        }
    );

    let idle_reaper = idle_reaper(state, idle_timeout_seconds);
    tokio::pin!(idle_reaper);
    let server_result = tokio::select! {
        result = &mut server_task => result,
        _ = wait_for_shutdown_signal() => {
            eprintln!("收到停止信号，正在关闭查看器。");
            let _ = shutdown_sender.send(());
            server_task.await
        }
        _ = &mut idle_reaper => {
            eprintln!("查看器已空闲超过时限，正在退出。");
            let _ = shutdown_sender.send(());
            server_task.await
        }
    };

    super::clear_server_record(&repository_root, std::process::id());
    match server_result {
        Ok(Ok(())) => 0,
        Ok(Err(error)) => {
            eprintln!("查看器服务异常退出：{error}");
            1
        }
        Err(error) => {
            eprintln!("查看器服务任务异常：{error}");
            1
        }
    }
}

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(root_page))
        .route("/assets/{asset}", get(static_asset))
        .route("/api/info", get(info))
        .route("/api/tree", get(tree))
        .route("/api/file", get(file_content))
        .route("/api/changes", get(changes))
        .route("/api/diff", get(diff))
        .route("/api/markdown", get(markdown))
        .route("/api/stage", post(stage))
        .route("/api/open", post(open_path))
        .route("/raw/{*path}", get(raw_file))
        .fallback(not_found)
        .layer(DefaultBodyLimit::max(WRITE_BODY_LIMIT))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            track_activity,
        ))
        .with_state(state)
}

async fn track_activity(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response<Body> {
    state.touch();
    next.run(request).await
}

async fn root_page() -> Response<Body> {
    serve_asset("index.html")
}

async fn static_asset(RoutePath(asset): RoutePath<String>) -> Response<Body> {
    serve_asset(&asset)
}

async fn info(State(state): State<AppState>) -> impl IntoResponse {
    json_response(workspace::info(&state.repository_root))
}

async fn tree(
    State(state): State<AppState>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let include_ignored = query
        .get("include_ignored")
        .is_some_and(|value| value == "1");
    json_response(workspace::tree(&state.repository_root, include_ignored))
}

async fn file_content(
    State(state): State<AppState>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    json_response(workspace::file_payload(
        &state.repository_root,
        query.get("path").map(String::as_str).unwrap_or_default(),
    ))
}

async fn changes(State(state): State<AppState>) -> impl IntoResponse {
    json_response(workspace::changes(&state.repository_root))
}

async fn diff(
    State(state): State<AppState>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    json_response(workspace::diff(
        &state.repository_root,
        query.get("path").map(String::as_str).unwrap_or_default(),
        query.get("section").map(String::as_str).unwrap_or_default(),
    ))
}

async fn markdown(
    State(state): State<AppState>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    json_response(workspace::markdown(
        &state.repository_root,
        query.get("path").map(String::as_str).unwrap_or_default(),
    ))
}

async fn stage(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let request = match parse_json_request(&headers, &body) {
        Ok(request) => request,
        Err(payload) => return json_response(payload),
    };
    json_response(workspace::stage(&state.repository_root, &request))
}

async fn open_path(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let request = match parse_json_request(&headers, &body) {
        Ok(request) => request,
        Err(payload) => return json_response(payload),
    };
    json_response(workspace::open(&state.repository_root, &request))
}

fn parse_json_request(headers: &HeaderMap, body: &[u8]) -> Result<Value, Payload> {
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .eq_ignore_ascii_case("application/json")
    {
        return Err(Payload::error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "拒绝：写接口只接受 application/json 请求体。",
        ));
    }
    if body.is_empty() {
        return Err(Payload::error(
            StatusCode::BAD_REQUEST,
            "拒绝：请求体为空。",
        ));
    }
    if body.len() > WRITE_BODY_LIMIT {
        return Err(Payload::error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "拒绝：请求体超过 64 KiB 上限。",
        ));
    }
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(object)) => Ok(Value::Object(object)),
        _ => Err(Payload::error(
            StatusCode::BAD_REQUEST,
            "拒绝：请求体必须是合法的 JSON 对象。",
        )),
    }
}

async fn raw_file(
    State(state): State<AppState>,
    RoutePath(path): RoutePath<String>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Response<Body> {
    match workspace::raw_file(
        &state.repository_root,
        &path,
        query.get("rev").map(String::as_str).unwrap_or_default(),
    ) {
        Ok((content_type, bytes)) => {
            let content_length = bytes.len().to_string();
            let mut response = Response::new(Body::from(bytes));
            *response.status_mut() = StatusCode::OK;
            set_header(response.headers_mut(), CONTENT_TYPE.as_str(), &content_type);
            set_header(
                response.headers_mut(),
                CONTENT_LENGTH.as_str(),
                &content_length,
            );
            set_header(response.headers_mut(), "X-Content-Type-Options", "nosniff");
            set_header(response.headers_mut(), CACHE_CONTROL.as_str(), "no-store");
            response
        }
        Err(payload) => json_response(payload),
    }
}

async fn not_found() -> impl IntoResponse {
    json_response(Payload::error(
        StatusCode::NOT_FOUND,
        "未知路由：本服务只提供白名单内的读取接口。",
    ))
}

fn serve_asset(asset_name: &str) -> Response<Body> {
    let content_type = match asset_name {
        "index.html" => "text/html; charset=utf-8",
        "viewer.css" => "text/css; charset=utf-8",
        "viewer-state.js" | "viewer-tree.js" | "viewer-content.js" | "viewer.js"
        | "mermaid.min.js" => "application/javascript; charset=utf-8",
        _ => {
            return json_response(Payload::error(StatusCode::NOT_FOUND, "未知的静态资源。"));
        }
    };
    let assets_directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/view/assets");
    let asset_bytes = match std::fs::read(assets_directory.join(asset_name)) {
        Ok(bytes) => bytes,
        Err(_) => return json_response(Payload::error(StatusCode::NOT_FOUND, "静态资源不存在。")),
    };
    let mut response = Response::new(Body::from(asset_bytes));
    *response.status_mut() = StatusCode::OK;
    set_header(response.headers_mut(), CONTENT_TYPE.as_str(), content_type);
    set_header(response.headers_mut(), CACHE_CONTROL.as_str(), "no-store");
    response
}

fn json_response(payload: Payload) -> Response<Body> {
    let mut response = (payload.status, Json(payload.body)).into_response();
    set_header(response.headers_mut(), CACHE_CONTROL.as_str(), "no-store");
    response
}

fn set_header(headers: &mut HeaderMap, name: &str, value: &str) {
    if let (Ok(name), Ok(value)) = (
        axum::http::header::HeaderName::from_bytes(name.as_bytes()),
        HeaderValue::from_str(value),
    ) {
        headers.insert(name, value);
    }
}

async fn idle_reaper(state: AppState, idle_timeout_seconds: f64) {
    if idle_timeout_seconds <= 0.0 {
        std::future::pending::<()>().await;
        return;
    }
    let timeout = Duration::from_secs(idle_timeout_seconds as u64);
    let check_interval = timeout
        .div_f64(4.0)
        .clamp(Duration::from_millis(100), Duration::from_secs(5));
    loop {
        sleep(check_interval).await;
        let idle_duration = state
            .last_request
            .lock()
            .map(|last_request| last_request.elapsed())
            .unwrap_or_default();
        if idle_duration.as_secs_f64() >= idle_timeout_seconds {
            return;
        }
    }
}

async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(signal) => signal,
                Err(_) => {
                    let _ = tokio::signal::ctrl_c().await;
                    return;
                }
            };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
