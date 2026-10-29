//! 管理后台路由的 HTTP 行为测试：走完整的路由与中间件，存储、会话、哈希换成内存替身。
//!
//! 重点是隔离（两套 Cookie/会话互不通用）、认证门槛、停用立即生效，以及参数校验与限流。
//! 真实 PostgreSQL/Redis 的端到端验证在 `services/accounts/tests/http_end_to_end.rs`。

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use tpl_api::{AppState, SessionCookieSettings, build_router};
use tpl_core::admin::testing::InMemoryAdminRepository;
use tpl_core::admin::{AdminPolicy, AdminPorts, AdminService};
use tpl_core::auth::testing::{
    InMemoryAttemptCounter, InMemorySessionStore, InMemoryUserRepository, PlainPasswordHasher,
};
use tpl_core::auth::{AuthPolicy, AuthPorts, AuthService, ThrottlePolicy};
use tpl_core::health::HealthService;

const WINDOW: Duration = Duration::from_secs(900);
const ADMIN_PASSWORD: &str = "admin-correct-horse";

fn throttle(per_account: u32) -> ThrottlePolicy {
    ThrottlePolicy {
        login_max_failures_per_ip: 1000,
        login_max_failures_per_account: per_account,
        login_window: WINDOW,
        register_max_per_ip: 1000,
        register_window: WINDOW,
    }
}

/// 普通域与管理员域共用用户表与计数器（和真实装配一致），会话存储各自独立。
async fn test_app(admin_per_account_limit: u32) -> Router {
    let users = Arc::new(InMemoryUserRepository::default());
    let counter = Arc::new(InMemoryAttemptCounter::default());
    let hasher = Arc::new(PlainPasswordHasher);
    let auth_service = AuthService::new(
        AuthPorts {
            user_repository: users.clone(),
            session_store: Arc::new(InMemorySessionStore::default()),
            password_hasher: hasher.clone(),
            attempt_counter: counter.clone(),
        },
        AuthPolicy {
            session_ttl: Duration::from_secs(3600),
            throttle: throttle(1000),
        },
    )
    .await
    .unwrap();
    let admin_service = AdminService::new(
        AdminPorts {
            admin_repository: Arc::new(InMemoryAdminRepository::default()),
            user_repository: users,
            session_store: Arc::new(InMemorySessionStore::default()),
            password_hasher: hasher,
            attempt_counter: counter,
        },
        AdminPolicy {
            session_ttl: Duration::from_secs(1800),
            throttle: throttle(admin_per_account_limit),
        },
    )
    .await
    .unwrap();
    admin_service
        .bootstrap_admin("root", ADMIN_PASSWORD)
        .await
        .unwrap();
    build_router(AppState {
        health_service: Arc::new(HealthService::new(vec![])),
        auth_service: Some(Arc::new(auth_service)),
        session_cookie: SessionCookieSettings {
            name: "session".to_string(),
            secure: false,
        },
        admin_service: Some(Arc::new(admin_service)),
        admin_cookie: SessionCookieSettings {
            name: "admin_session".to_string(),
            secure: true,
        },
        trust_forwarded_for: false,
    })
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

impl Reply {
    fn set_cookie(&self) -> Option<String> {
        self.headers
            .get(header::SET_COOKIE)
            .map(|value| value.to_str().unwrap().to_string())
    }

    fn cookie_to_send_back(&self) -> Option<String> {
        self.set_cookie()
            .map(|set_cookie| set_cookie.split(';').next().unwrap().to_string())
    }
}

async fn send(router: &Router, request: Request<Body>) -> Reply {
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    Reply {
        status,
        headers,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    }
}

fn post_json(uri: &str, body: &Value, cookie: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

fn get(uri: &str, cookie: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().uri(uri);
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    builder.body(Body::empty()).unwrap()
}

async fn admin_login(router: &Router) -> String {
    let reply = send(
        router,
        post_json(
            "/admin/auth/login",
            &json!({"identifier": "root", "password": ADMIN_PASSWORD}),
            None,
        ),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.body);
    reply.cookie_to_send_back().expect("登录应设置 Cookie")
}

/// 通过公开接口注册一个普通用户，返回其 ID。
async fn register_user(router: &Router, email: &str) -> String {
    let reply = send(
        router,
        post_json(
            "/auth/register",
            &json!({"email": email, "password": "correct-horse"}),
            None,
        ),
    )
    .await;
    assert_eq!(reply.status, StatusCode::CREATED);
    reply.body["user"]["id"].as_i64().unwrap().to_string()
}

async fn user_login(router: &Router, email: &str) -> Reply {
    send(
        router,
        post_json(
            "/auth/login",
            &json!({"email": email, "password": "correct-horse"}),
            None,
        ),
    )
    .await
}

#[tokio::test]
async fn login_sets_an_httponly_admin_cookie_and_returns_the_admin() {
    let router = test_app(1000).await;
    let reply = send(
        &router,
        post_json(
            "/admin/auth/login",
            &json!({"identifier": " ROOT ", "password": ADMIN_PASSWORD}),
            None,
        ),
    )
    .await;

    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.body["username"], "root");
    assert_eq!(reply.body["display_name"], "root");
    assert!(
        reply.body["user_id"].is_string(),
        "ID 用字符串，避免前端精度问题"
    );
    assert!(reply.body.get("password").is_none() && reply.body.get("password_hash").is_none());
    let cookie = reply.set_cookie().unwrap();
    assert!(cookie.starts_with("admin_session="), "{cookie}");
    for attribute in [
        "HttpOnly",
        "SameSite=Lax",
        "Secure",
        "Path=/",
        "Max-Age=1800",
    ] {
        assert!(cookie.contains(attribute), "缺少 {attribute}：{cookie}");
    }

    let me = send(
        &router,
        get("/admin/auth/me", reply.cookie_to_send_back().as_deref()),
    )
    .await;
    assert_eq!(
        (me.status, me.body["username"].clone()),
        (StatusCode::OK, json!("root"))
    );
}

#[tokio::test]
async fn every_admin_route_requires_an_admin_session() {
    let router = test_app(1000).await;
    for request in [
        get("/admin/auth/me", None),
        get("/admin/users", None),
        get("/admin/users/1", None),
        post_json("/admin/users/1/disable", &json!({}), None),
        post_json("/admin/users/1/enable", &json!({}), None),
        get("/admin/users", Some("admin_session=forged-token")),
    ] {
        let reply = send(&router, request).await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
        assert_eq!(reply.body["error"]["code"], "unauthenticated");
    }
}

#[tokio::test]
async fn user_and_admin_sessions_are_not_interchangeable() {
    let router = test_app(1000).await;
    register_user(&router, "alice@example.com").await;
    // 内存替身的令牌是递增编号，两个会话库里同一个编号会互相「撞上」。
    // 这里让用户库只剩编号 1、管理员库只剩编号 0，伪造出来的同名 Cookie 才不会碰巧有效。
    let first_user = user_login(&router, "alice@example.com").await;
    let user_cookie = user_login(&router, "alice@example.com")
        .await
        .cookie_to_send_back()
        .unwrap();
    let logout_first = post_json(
        "/auth/logout",
        &json!({}),
        first_user.cookie_to_send_back().as_deref(),
    );
    assert_eq!(
        send(&router, logout_first).await.status,
        StatusCode::NO_CONTENT
    );
    let admin_cookie = admin_login(&router).await;
    let spare_admin = admin_login(&router).await;
    let logout_spare = post_json("/admin/auth/logout", &json!({}), Some(&spare_admin));
    assert_eq!(
        send(&router, logout_spare).await.status,
        StatusCode::NO_CONTENT
    );

    // 普通用户的 Cookie：换个名字冒充管理员、或原样带着，都进不了后台。
    let forged_name = user_cookie.replacen("session=", "admin_session=", 1);
    for cookie in [user_cookie.as_str(), forged_name.as_str()] {
        let reply = send(&router, get("/admin/users", Some(cookie))).await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{cookie}");
    }
    // 管理员的 Cookie 在普通域同样没用。
    let as_user = admin_cookie.replacen("admin_session=", "session=", 1);
    let reply = send(&router, get("/auth/me", Some(&as_user))).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    // 两套 Cookie 可以同时存在于一个浏览器里，各管各的。
    let both = format!("{user_cookie}; {admin_cookie}");
    assert_eq!(
        send(&router, get("/auth/me", Some(&both))).await.status,
        StatusCode::OK
    );
    assert_eq!(
        send(&router, get("/admin/users", Some(&both))).await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn unknown_username_and_wrong_password_look_identical() {
    let router = test_app(1000).await;
    let wrong = send(
        &router,
        post_json(
            "/admin/auth/login",
            &json!({"identifier": "root", "password": "nope-nope-nope"}),
            None,
        ),
    )
    .await;
    let unknown = send(
        &router,
        post_json(
            "/admin/auth/login",
            &json!({"identifier": "nobody", "password": ADMIN_PASSWORD}),
            None,
        ),
    )
    .await;

    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    assert_eq!((wrong.status, &wrong.body), (unknown.status, &unknown.body));
    assert_eq!(wrong.body["error"]["code"], "invalid_credentials");
    assert!(wrong.set_cookie().is_none());
}

#[tokio::test]
async fn admin_login_is_throttled_without_locking_out_ordinary_users() {
    let router = test_app(3).await;
    register_user(&router, "root@example.com").await;
    for _ in 0..3 {
        let reply = send(
            &router,
            post_json(
                "/admin/auth/login",
                &json!({"identifier": "root", "password": "wrong-wrong"}),
                None,
            ),
        )
        .await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    }

    let blocked = send(
        &router,
        post_json(
            "/admin/auth/login",
            &json!({"identifier": "root", "password": ADMIN_PASSWORD}),
            None,
        ),
    )
    .await;
    assert_eq!(blocked.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(blocked.body["error"]["code"], "too_many_attempts");
    assert!(blocked.headers.get(header::RETRY_AFTER).is_some());
    assert_eq!(
        user_login(&router, "root@example.com").await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn logout_invalidates_the_session_and_is_idempotent() {
    let router = test_app(1000).await;
    let cookie = admin_login(&router).await;

    let out = send(
        &router,
        post_json("/admin/auth/logout", &json!({}), Some(&cookie)),
    )
    .await;
    assert_eq!(out.status, StatusCode::NO_CONTENT);
    assert!(out.set_cookie().unwrap().contains("Max-Age=0"));
    assert_eq!(
        send(&router, get("/admin/auth/me", Some(&cookie)))
            .await
            .status,
        StatusCode::UNAUTHORIZED,
        "退出后旧令牌必须失效"
    );
    let again = send(&router, post_json("/admin/auth/logout", &json!({}), None)).await;
    assert_eq!(again.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn listing_paginates_filters_and_validates_parameters() {
    let router = test_app(1000).await;
    for email in ["a@x.com", "b@x.com", "c@y.org"] {
        register_user(&router, email).await;
    }
    let cookie = admin_login(&router).await;
    let list = |query: &str| get(&format!("/admin/users{query}"), Some(&cookie));

    let page = send(&router, list("?page=1&page_size=2")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert_eq!(
        (
            page.body["total"].clone(),
            page.body["page"].clone(),
            page.body["page_size"].clone()
        ),
        (json!(3), json!(1), json!(2))
    );
    let first = &page.body["items"][0];
    assert_eq!(
        (first["email"].clone(), first["status"].clone()),
        (json!("a@x.com"), json!("active"))
    );
    assert!(first["id"].is_string() && first["created_at"].is_string());
    assert!(first.get("password_hash").is_none());
    assert_eq!(page.body["items"].as_array().unwrap().len(), 2);

    let by_keyword = send(&router, list("?keyword=X.COM")).await;
    assert_eq!(by_keyword.body["total"], 2);
    let clamped = send(&router, list("?page=0&page_size=100000")).await;
    assert_eq!(
        (
            clamped.body["page"].clone(),
            clamped.body["page_size"].clone()
        ),
        (json!(1), json!(100))
    );
    // 空的 status 当作没有过滤（前端「全部」选项）。
    assert_eq!(send(&router, list("?status=")).await.body["total"], 3);

    for bad in ["?status=banned", "?page=abc"] {
        let reply = send(&router, list(bad)).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{bad}");
    }
}

#[tokio::test]
async fn disabling_takes_effect_immediately_and_enabling_restores_access() {
    let router = test_app(1000).await;
    let user_id = register_user(&router, "alice@example.com").await;
    let user_cookie = user_login(&router, "alice@example.com")
        .await
        .cookie_to_send_back()
        .unwrap();
    let admin_cookie = admin_login(&router).await;
    assert_eq!(
        send(&router, get("/auth/me", Some(&user_cookie)))
            .await
            .status,
        StatusCode::OK
    );

    let disabled = send(
        &router,
        post_json(
            &format!("/admin/users/{user_id}/disable"),
            &json!({}),
            Some(&admin_cookie),
        ),
    )
    .await;
    assert_eq!(disabled.status, StatusCode::OK);
    assert_eq!(disabled.body["status"], "disabled");

    // 已有的登录态立刻失效；新登录被拒，且和「密码错」是不同的错误码。
    let me = send(&router, get("/auth/me", Some(&user_cookie))).await;
    assert_eq!(
        (me.status, me.body["error"]["code"].clone()),
        (StatusCode::UNAUTHORIZED, json!("unauthenticated"))
    );
    let login = user_login(&router, "alice@example.com").await;
    assert_eq!(
        (login.status, login.body["error"]["code"].clone()),
        (StatusCode::FORBIDDEN, json!("account_disabled"))
    );
    assert!(login.set_cookie().is_none());
    let listed = send(
        &router,
        get("/admin/users?status=disabled", Some(&admin_cookie)),
    )
    .await;
    assert_eq!(listed.body["total"], 1);

    // 重复停用幂等；启用后恢复，原来的会话也重新有效（数据没有被删）。
    let again = send(
        &router,
        post_json(
            &format!("/admin/users/{user_id}/disable"),
            &json!({}),
            Some(&admin_cookie),
        ),
    )
    .await;
    assert_eq!(again.status, StatusCode::OK);
    let enabled = send(
        &router,
        post_json(
            &format!("/admin/users/{user_id}/enable"),
            &json!({}),
            Some(&admin_cookie),
        ),
    )
    .await;
    assert_eq!(enabled.body["status"], "active");
    assert_eq!(
        user_login(&router, "alice@example.com").await.status,
        StatusCode::OK
    );
    assert_eq!(
        send(&router, get("/auth/me", Some(&user_cookie)))
            .await
            .status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn unknown_and_malformed_user_ids_are_404() {
    let router = test_app(1000).await;
    let cookie = admin_login(&router).await;
    for id in ["999", "abc", "1.5", "-1"] {
        for request in [
            get(&format!("/admin/users/{id}"), Some(&cookie)),
            post_json(
                &format!("/admin/users/{id}/disable"),
                &json!({}),
                Some(&cookie),
            ),
            post_json(
                &format!("/admin/users/{id}/enable"),
                &json!({}),
                Some(&cookie),
            ),
        ] {
            let reply = send(&router, request).await;
            assert_eq!(reply.status, StatusCode::NOT_FOUND, "{id}");
            assert_eq!(reply.body["error"]["code"], "not_found");
        }
    }
}

#[tokio::test]
async fn state_changing_requests_must_be_json() {
    let router = test_app(1000).await;
    let user_id = register_user(&router, "alice@example.com").await;
    let cookie = admin_login(&router).await;

    // 浏览器跨站表单发不出 application/json，所以拒绝其它类型就是 CSRF 防线的一部分。
    let form_login = Request::builder()
        .method("POST")
        .uri("/admin/auth/login")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("identifier=root&password=x"))
        .unwrap();
    assert_eq!(
        send(&router, form_login).await.status,
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    let no_content_type = Request::builder()
        .method("POST")
        .uri(format!("/admin/users/{user_id}/disable"))
        .header(header::COOKIE, &cookie)
        .body(Body::empty())
        .unwrap();
    let reply = send(&router, no_content_type).await;
    assert_eq!(reply.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    // 没有真的被停用。
    let user = send(
        &router,
        get(&format!("/admin/users/{user_id}"), Some(&cookie)),
    )
    .await;
    assert_eq!(user.body["status"], "active");
}

#[tokio::test]
async fn admin_routes_answer_503_when_admin_is_not_configured() {
    let router = build_router(AppState {
        health_service: Arc::new(HealthService::new(vec![])),
        auth_service: None,
        session_cookie: SessionCookieSettings {
            name: "session".into(),
            secure: false,
        },
        admin_service: None,
        admin_cookie: SessionCookieSettings {
            name: "admin_session".into(),
            secure: false,
        },
        trust_forwarded_for: false,
    });
    for request in [
        post_json(
            "/admin/auth/login",
            &json!({"identifier": "root", "password": "x"}),
            None,
        ),
        post_json("/admin/auth/logout", &json!({}), None),
        get("/admin/auth/me", None),
        get("/admin/users", None),
    ] {
        let reply = send(&router, request).await;
        assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(reply.body["error"]["code"], "admin_unavailable");
    }
}
