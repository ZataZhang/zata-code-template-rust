//! 管理后台路由：`/admin/auth/*`（登录、退出、当前管理员）与 `/admin/users*`（用户管理）。
//!
//! 与普通用户路由完全隔离：独立的 Cookie（名字不同）、独立的会话存储、独立的限流。
//! 管理员**没有注册入口**，只能由启动时的引导账号创建。
//! CSRF 的防护思路与 [`crate::auth`] 相同（`SameSite=Lax` + 写接口只收 JSON，且不开放 CORS）。
//! 注意 `enable`/`disable` 这类写接口即使没有请求体，也必须带 `Content-Type: application/json`。

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{
    ConnectInfo, DefaultBodyLimit, Extension, FromRequestParts, Path, Query, State,
};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::cookie::CookieJar;
use serde::{Deserialize, Serialize};
use tpl_core::admin::{Admin, AdminService, UserListRequest};
use tpl_core::auth::{AuthError, User, UserStatus};

use crate::AppState;
use crate::http::{
    ApiError, MAX_REQUEST_BODY_BYTES, build_removal_cookie, build_session_cookie, client_ip,
    session_token_from,
};

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/admin/auth/login", post(login))
        .route("/admin/auth/logout", post(logout))
        .route("/admin/auth/me", get(current_admin))
        .route("/admin/users", get(list_users))
        .route("/admin/users/{id}", get(get_user))
        .route("/admin/users/{id}/enable", post(enable_user))
        .route("/admin/users/{id}/disable", post(disable_user))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
}

fn admin_service(app_state: &AppState) -> Result<&Arc<AdminService>, ApiError> {
    app_state
        .admin_service
        .as_ref()
        .ok_or_else(ApiError::admin_unavailable)
}

/// 已登录的管理员：用在需要管理员身份的处理函数的参数里，没有有效的管理员会话就直接 401。
struct AdminSession(#[allow(dead_code, reason = "目前的处理函数只需要「已登录」这个事实")] Admin);

impl FromRequestParts<AppState> for AdminSession {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        app_state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let admin_service = admin_service(app_state)?;
        let cookie_jar = CookieJar::from_headers(&parts.headers);
        let session_token = session_token_from(&cookie_jar, &app_state.admin_cookie)
            .ok_or(AuthError::Unauthenticated)?;
        Ok(Self(admin_service.authenticate(&session_token).await?))
    }
}

/// 要求请求的 `Content-Type` 是 `application/json`，否则 415。
///
/// 没有请求体的写接口（启用、停用、退出）用 `Json` 提取器拦不住跨站表单，所以单独检查：
/// 跨站表单发不出这种类型，要发 JSON 就需要 CORS 预检，而本服务不开放 CORS。
struct RequireJson;

impl<S: Send + Sync> FromRequestParts<S> for RequireJson {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let is_json = parts
            .headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("application/json"));
        if is_json {
            Ok(Self)
        } else {
            Err(ApiError::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported_media_type",
                "写请求必须使用 Content-Type: application/json",
            ))
        }
    }
}

/// 管理员登录请求。故意不派生 `Debug`，避免密码被意外打印进日志。
#[derive(Deserialize)]
struct LoginRequest {
    /// 登录标识：管理员用户名。
    identifier: String,
    password: String,
}

#[derive(Serialize)]
struct AdminResponse {
    user_id: String,
    display_name: String,
    username: String,
}

impl From<Admin> for AdminResponse {
    fn from(admin: Admin) -> Self {
        Self {
            user_id: admin.id.0.to_string(),
            display_name: admin.display_name,
            username: admin.username,
        }
    }
}

#[derive(Serialize)]
struct UserResponse {
    id: String,
    email: String,
    status: &'static str,
    created_at: String,
}

impl From<User> for UserResponse {
    fn from(user: User) -> Self {
        Self {
            id: user.id.0.to_string(),
            email: user.email,
            status: user.status.as_str(),
            created_at: user.created_at,
        }
    }
}

async fn login(
    State(app_state): State<AppState>,
    headers: HeaderMap,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    cookie_jar: CookieJar,
    Json(request): Json<LoginRequest>,
) -> Result<(CookieJar, Json<AdminResponse>), ApiError> {
    let admin_service = admin_service(&app_state)?;
    let client_ip = client_ip(
        &app_state,
        &headers,
        peer.map(|Extension(ConnectInfo(address))| address),
    );
    let (admin, session_token) = admin_service
        .login(&request.identifier, &request.password, client_ip)
        .await?;

    // 登录总是签发新令牌（防会话固定）；带着的旧令牌顺手作废，失败不影响本次登录。
    if let Some(previous_token) = session_token_from(&cookie_jar, &app_state.admin_cookie)
        && let Err(revoke_error) = admin_service.logout(&previous_token).await
    {
        tracing::warn!(error = %revoke_error, "作废旧管理员会话失败");
    }

    tracing::info!(admin_id = admin.id.0, "管理员登录成功");
    let cookie = build_session_cookie(
        &app_state.admin_cookie,
        &session_token,
        admin_service.session_ttl(),
    );
    Ok((cookie_jar.add(cookie), Json(admin.into())))
}

async fn logout(
    State(app_state): State<AppState>,
    _json: RequireJson,
    cookie_jar: CookieJar,
) -> Result<(StatusCode, CookieJar), ApiError> {
    let admin_service = admin_service(&app_state)?;
    if let Some(session_token) = session_token_from(&cookie_jar, &app_state.admin_cookie) {
        admin_service.logout(&session_token).await?;
    }
    // 没带令牌也返回成功并清除 Cookie：退出是幂等的。
    Ok((
        StatusCode::NO_CONTENT,
        cookie_jar.add(build_removal_cookie(&app_state.admin_cookie)),
    ))
}

async fn current_admin(AdminSession(admin): AdminSession) -> Json<AdminResponse> {
    Json(admin.into())
}

/// 列表的查询参数。取值非法时返回 400，而不是悄悄忽略。
#[derive(Deserialize)]
struct ListParams {
    page: Option<u32>,
    page_size: Option<u32>,
    status: Option<String>,
    keyword: Option<String>,
}

#[derive(Serialize)]
struct UserListResponse {
    items: Vec<UserResponse>,
    total: u64,
    page: u32,
    page_size: u32,
}

async fn list_users(
    State(app_state): State<AppState>,
    _admin: AdminSession,
    Query(params): Query<ListParams>,
) -> Result<Json<UserListResponse>, ApiError> {
    let status = params
        .status
        .as_deref()
        .filter(|text| !text.is_empty())
        .map(|text| {
            UserStatus::parse(text)
                .ok_or_else(|| ApiError::invalid_parameter("status 只能是 active 或 disabled"))
        })
        .transpose()?;
    let admin_service = admin_service(&app_state)?;
    let page = admin_service
        .list_users(UserListRequest {
            page: params.page,
            page_size: params.page_size,
            status,
            keyword: params.keyword,
        })
        .await?;
    Ok(Json(UserListResponse {
        items: page.items.into_iter().map(UserResponse::from).collect(),
        total: page.total,
        // 回显服务端实际采用的页码与每页条数（越界值已被规整），前端据此翻页。
        page: params.page.unwrap_or(1).max(1),
        page_size: params
            .page_size
            .unwrap_or(tpl_core::admin::DEFAULT_PAGE_SIZE)
            .clamp(1, tpl_core::admin::MAX_PAGE_SIZE),
    }))
}

/// 路径里的用户 ID 不是数字时，按「用户不存在」处理。
fn parse_user_id(raw_id: &str) -> Result<i64, ApiError> {
    raw_id.parse().map_err(|_| ApiError::not_found("用户"))
}

async fn get_user(
    State(app_state): State<AppState>,
    _admin: AdminSession,
    Path(raw_id): Path<String>,
) -> Result<Json<UserResponse>, ApiError> {
    let user = admin_service(&app_state)?
        .get_user(parse_user_id(&raw_id)?)
        .await?
        .ok_or_else(|| ApiError::not_found("用户"))?;
    Ok(Json(user.into()))
}

async fn change_status(
    app_state: &AppState,
    raw_id: &str,
    status: UserStatus,
) -> Result<Json<UserResponse>, ApiError> {
    let user_id = parse_user_id(raw_id)?;
    let user = admin_service(app_state)?
        .set_user_status(user_id, status)
        .await?
        .ok_or_else(|| ApiError::not_found("用户"))?;
    tracing::info!(user_id, status = status.as_str(), "管理员修改了用户状态");
    Ok(Json(user.into()))
}

async fn enable_user(
    State(app_state): State<AppState>,
    _json: RequireJson,
    _admin: AdminSession,
    Path(raw_id): Path<String>,
) -> Result<Json<UserResponse>, ApiError> {
    change_status(&app_state, &raw_id, UserStatus::Active).await
}

async fn disable_user(
    State(app_state): State<AppState>,
    _json: RequireJson,
    _admin: AdminSession,
    Path(raw_id): Path<String>,
) -> Result<Json<UserResponse>, ApiError> {
    change_status(&app_state, &raw_id, UserStatus::Disabled).await
}
