//! 认证路由：`/auth/register`、`/auth/login`、`/auth/logout`、`/auth/me`。
//!
//! 会话用 HTTP-only Cookie 携带，令牌对 JavaScript 不可见（防 XSS 窃取）。
//!
//! 关于 CSRF：Cookie 设置了 `SameSite=Lax`，浏览器不会在跨站 POST 里带上它；
//! 并且所有写接口只接受 `Content-Type: application/json`，跨站表单无法构造这种请求
//! （跨站发 JSON 需要 CORS 预检，本服务不开放 CORS）。因此暂不需要额外的 CSRF 令牌。
//! **如果将来开放 CORS，或增加接受表单编码的写接口，必须重新评估这一点。**

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, DefaultBodyLimit, Extension, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::cookie::CookieJar;
use serde::{Deserialize, Serialize};
use tpl_core::auth::{AuthError, AuthService, User};

use crate::AppState;
use crate::http::{
    ApiError, MAX_REQUEST_BODY_BYTES, build_removal_cookie, build_session_cookie, client_ip,
    session_token_from,
};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/me", get(current_user))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
}

/// 注册和登录共用的请求体。故意不派生 `Debug`，避免密码被意外打印进日志。
#[derive(Deserialize)]
struct CredentialsRequest {
    email: String,
    password: String,
}

#[derive(Serialize)]
struct UserResponse {
    id: i64,
    email: String,
}

#[derive(Serialize)]
struct UserEnvelope {
    user: UserResponse,
}

impl From<User> for UserEnvelope {
    fn from(user: User) -> Self {
        Self {
            user: UserResponse {
                id: user.id.0,
                email: user.email,
            },
        }
    }
}

fn auth_service(app_state: &AppState) -> Result<&Arc<AuthService>, ApiError> {
    app_state
        .auth_service
        .as_ref()
        .ok_or_else(ApiError::auth_unavailable)
}

async fn register(
    State(app_state): State<AppState>,
    headers: HeaderMap,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    Json(request): Json<CredentialsRequest>,
) -> Result<(StatusCode, Json<UserEnvelope>), ApiError> {
    let auth_service = auth_service(&app_state)?;
    let client_ip = client_ip(
        &app_state,
        &headers,
        peer.map(|Extension(ConnectInfo(address))| address),
    );
    let new_user = auth_service
        .register(&request.email, &request.password, client_ip)
        .await?;
    tracing::info!(user_id = new_user.id.0, "用户注册成功");
    Ok((StatusCode::CREATED, Json(new_user.into())))
}

async fn login(
    State(app_state): State<AppState>,
    headers: HeaderMap,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    cookie_jar: CookieJar,
    Json(request): Json<CredentialsRequest>,
) -> Result<(CookieJar, Json<UserEnvelope>), ApiError> {
    let auth_service = auth_service(&app_state)?;
    let client_ip = client_ip(
        &app_state,
        &headers,
        peer.map(|Extension(ConnectInfo(address))| address),
    );
    let (user, session_token) = auth_service
        .login(&request.email, &request.password, client_ip)
        .await?;

    // 登录总是签发新令牌（防会话固定）；请求里带着的旧令牌顺手作废，失败也不影响本次登录。
    if let Some(previous_token) = session_token_from(&cookie_jar, &app_state.session_cookie)
        && let Err(revoke_error) = auth_service.logout(&previous_token).await
    {
        tracing::warn!(error = %revoke_error, "作废旧会话失败");
    }

    tracing::info!(user_id = user.id.0, "用户登录成功");
    let session_cookie = build_session_cookie(
        &app_state.session_cookie,
        &session_token,
        auth_service.session_ttl(),
    );
    Ok((cookie_jar.add(session_cookie), Json(user.into())))
}

async fn logout(
    State(app_state): State<AppState>,
    cookie_jar: CookieJar,
) -> Result<(StatusCode, CookieJar), ApiError> {
    let auth_service = auth_service(&app_state)?;
    if let Some(session_token) = session_token_from(&cookie_jar, &app_state.session_cookie) {
        auth_service.logout(&session_token).await?;
    }
    // 没带令牌也返回成功并清除 Cookie：退出是幂等的。
    Ok((
        StatusCode::NO_CONTENT,
        cookie_jar.add(build_removal_cookie(&app_state.session_cookie)),
    ))
}

async fn current_user(
    State(app_state): State<AppState>,
    cookie_jar: CookieJar,
) -> Result<Json<UserEnvelope>, ApiError> {
    let auth_service = auth_service(&app_state)?;
    let session_token = session_token_from(&cookie_jar, &app_state.session_cookie)
        .ok_or(AuthError::Unauthenticated)?;
    let user = auth_service.authenticate(&session_token).await?;
    Ok(Json(user.into()))
}
