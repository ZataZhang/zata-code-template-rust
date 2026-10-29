//! 接入层（api）。
//!
//! 接收 HTTP 请求，只做三件事：参数校验、DTO 转换、调用 `tpl-core` 的用例。
//! 不写业务规则；只依赖 `tpl-core`，因此碰不到数据库等具体实现（有人把 infrastructure 加进它的依赖，`tests/architecture.rs` 会失败）。

mod admin;
mod auth;
mod health;
mod http;

use std::sync::Arc;

use axum::Router;
use tower_http::trace::TraceLayer;
use tpl_core::admin::AdminService;
use tpl_core::auth::AuthService;
use tpl_core::health::HealthService;

/// 会话 Cookie 的属性，由账号服务根据配置装配。
#[derive(Debug, Clone)]
pub struct SessionCookieSettings {
    /// Cookie 名。
    pub name: String,
    /// 是否带 `Secure` 属性（只经 HTTPS 发送）。本地 HTTP 开发为 `false`，生产必须为 `true`。
    pub secure: bool,
}

/// 所有路由共享的应用状态，由账号服务在启动时装配。
#[derive(Clone)]
pub struct AppState {
    /// 健康检查用例。
    pub health_service: Arc<HealthService>,
    /// 认证用例；`None` 表示未配置数据库或 Redis，认证相关路由返回 503。
    pub auth_service: Option<Arc<AuthService>>,
    /// 会话 Cookie 属性。
    pub session_cookie: SessionCookieSettings,
    /// 管理员用例；`None` 表示未配置数据库或 Redis，`/admin/*` 返回 503。
    pub admin_service: Option<Arc<AdminService>>,
    /// 管理员会话 Cookie 属性：名字必须与 `session_cookie` 不同，两者才能互不干扰。
    pub admin_cookie: SessionCookieSettings,
    /// 是否信任 `X-Forwarded-For` 来识别客户端 IP（用于限流）。
    ///
    /// 默认应为 `false`。只有服务确实部署在会追加该头的反向代理之后、且外部无法绕过代理直连时才能开启，
    /// 否则攻击者可以随意填这个头来绕过按 IP 的限流。
    pub trust_forwarded_for: bool,
}

/// 构建完整的路由，并挂上请求日志中间件。
pub fn build_router(app_state: AppState) -> Router {
    Router::new()
        .merge(health::routes())
        .merge(auth::routes())
        .merge(admin::routes())
        .layer(TraceLayer::new_for_http())
        .with_state(app_state)
}
