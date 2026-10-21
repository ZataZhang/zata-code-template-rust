//! 认证路由的 HTTP 行为测试：走完整的路由与中间件，只把用户存储、会话存储、密码哈希换成内存替身。
//!
//! 浏览器行为（带上 Cookie）在这里用辅助函数模拟；真实 socket + 真实 PostgreSQL/Redis 的
//! 端到端验证在 `tpl-app` 的 `tests/http_end_to_end.rs`。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use tpl_api::{AppState, SessionCookieSettings, build_router};
use tpl_core::auth::testing::{
    InMemoryAttemptCounter, InMemorySessionStore, InMemoryUserRepository, PlainPasswordHasher,
};
use tpl_core::auth::{AuthPolicy, AuthPorts, AuthService, ThrottlePolicy};
use tpl_core::health::HealthService;

const SESSION_TTL_SECS: u64 = 3600;
const THROTTLE_WINDOW: Duration = Duration::from_secs(900);

struct TestApp {
    router: Router,
    session_store: Arc<InMemorySessionStore>,
}

/// 限制宽松的限流策略：让与限流无关的测试不被干扰。
fn generous_throttle() -> ThrottlePolicy {
    ThrottlePolicy {
        login_max_failures_per_ip: 1000,
        login_max_failures_per_account: 1000,
        login_window: THROTTLE_WINDOW,
        register_max_per_ip: 1000,
        register_window: THROTTLE_WINDOW,
    }
}

async fn test_app(cookie_secure: bool) -> TestApp {
    test_app_with(cookie_secure, generous_throttle(), false).await
}

async fn test_app_with(
    cookie_secure: bool,
    throttle: ThrottlePolicy,
    trust_forwarded_for: bool,
) -> TestApp {
    let session_store = Arc::new(InMemorySessionStore::default());
    let auth_service = AuthService::new(
        AuthPorts {
            user_repository: Arc::new(InMemoryUserRepository::default()),
            session_store: session_store.clone(),
            password_hasher: Arc::new(PlainPasswordHasher),
            attempt_counter: Arc::new(InMemoryAttemptCounter::default()),
        },
        AuthPolicy {
            session_ttl: Duration::from_secs(SESSION_TTL_SECS),
            throttle,
        },
    )
    .await
    .unwrap();
    let router = build_router(AppState {
        health_service: Arc::new(HealthService::new(vec![])),
        auth_service: Some(Arc::new(auth_service)),
        session_cookie: SessionCookieSettings {
            name: "session".to_string(),
            secure: cookie_secure,
        },
        admin_service: None,
        admin_cookie: SessionCookieSettings {
            name: "admin_session".to_string(),
            secure: cookie_secure,
        },
        trust_forwarded_for,
    });
    TestApp {
        router,
        session_store,
    }
}

/// 模拟这个请求来自某个 TCP 对端地址（真实服务里由 `into_make_service_with_connect_info` 注入）。
fn from_peer(mut request: Request<Body>, peer: &str) -> Request<Body> {
    let address: SocketAddr = format!("{peer}:40000").parse().unwrap();
    request.extensions_mut().insert(ConnectInfo(address));
    request
}

/// 附加请求头。
fn with_header(mut request: Request<Body>, name: &'static str, value: &str) -> Request<Body> {
    request.headers_mut().append(name, value.parse().unwrap());
    request
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

impl Reply {
    /// 取出 `Set-Cookie` 头的完整内容（测试里每个响应最多设置一个 Cookie）。
    fn set_cookie(&self) -> Option<String> {
        self.headers
            .get(header::SET_COOKIE)
            .map(|value| value.to_str().unwrap().to_string())
    }

    /// 模拟浏览器：只把 `name=value` 部分回传给服务器。
    fn cookie_to_send_back(&self) -> Option<String> {
        self.set_cookie()
            .map(|set_cookie| set_cookie.split(';').next().unwrap().to_string())
    }
}

async fn send(router: &Router, request: Request<Body>) -> Reply {
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&body_bytes).unwrap_or(Value::Null);
    Reply {
        status,
        headers,
        body,
    }
}

fn json_post(uri: &str, body: &Value, cookie: Option<&str>) -> Request<Body> {
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

fn credentials(email: &str, password: &str) -> Value {
    json!({"email": email, "password": password})
}

async fn register_and_login(app: &TestApp, email: &str) -> Reply {
    let registered = send(
        &app.router,
        json_post("/auth/register", &credentials(email, "correct-horse"), None),
    )
    .await;
    assert_eq!(registered.status, StatusCode::CREATED);
    send(
        &app.router,
        json_post("/auth/login", &credentials(email, "correct-horse"), None),
    )
    .await
}

#[tokio::test]
async fn register_returns_the_normalised_user_and_does_not_log_in() {
    let app = test_app(false).await;
    let reply = send(
        &app.router,
        json_post(
            "/auth/register",
            &credentials("  Alice@Example.COM ", "correct-horse"),
            None,
        ),
    )
    .await;

    assert_eq!(reply.status, StatusCode::CREATED);
    assert_eq!(reply.body["user"]["email"], "alice@example.com");
    assert!(reply.body["user"]["id"].is_i64());
    assert!(reply.set_cookie().is_none(), "注册不应自动登录");
    assert_eq!(app.session_store.active_session_count(), 0);
}

#[tokio::test]
async fn responses_never_contain_passwords_or_hashes() {
    let app = test_app(false).await;
    let registered = send(
        &app.router,
        json_post(
            "/auth/register",
            &credentials("a@example.com", "correct-horse"),
            None,
        ),
    )
    .await;
    let logged_in = register_and_login(&app, "b@example.com").await;
    let me = send(
        &app.router,
        get("/auth/me", logged_in.cookie_to_send_back().as_deref()),
    )
    .await;

    for reply in [registered, logged_in, me] {
        let rendered = reply.body.to_string();
        assert!(!rendered.contains("correct-horse"));
        assert!(!rendered.contains("plain$"), "不能返回密码哈希：{rendered}");
        assert!(!rendered.contains("password"));
    }
}

#[tokio::test]
async fn register_rejects_bad_input_with_specific_error_codes() {
    let app = test_app(false).await;
    for (email, password, expected_code) in [
        ("not-an-email", "correct-horse", "invalid_email"),
        ("a@example.com", "short", "password_too_short"),
        ("a@example.com", &"x".repeat(200), "password_too_long"),
    ] {
        let reply = send(
            &app.router,
            json_post("/auth/register", &credentials(email, password), None),
        )
        .await;
        assert_eq!(
            reply.status,
            StatusCode::BAD_REQUEST,
            "{email} / {expected_code}"
        );
        assert_eq!(reply.body["error"]["code"], expected_code);
    }
}

#[tokio::test]
async fn registering_a_duplicate_email_is_409_regardless_of_case() {
    let app = test_app(false).await;
    let first = credentials("dup@example.com", "correct-horse");
    assert_eq!(
        send(&app.router, json_post("/auth/register", &first, None))
            .await
            .status,
        StatusCode::CREATED
    );

    let second = credentials("DUP@example.com", "another-password");
    let reply = send(&app.router, json_post("/auth/register", &second, None)).await;
    assert_eq!(reply.status, StatusCode::CONFLICT);
    assert_eq!(reply.body["error"]["code"], "email_taken");
}

#[tokio::test]
async fn login_sets_a_hardened_session_cookie() {
    let app = test_app(false).await;
    let reply = register_and_login(&app, "carol@example.com").await;

    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.body["user"]["email"], "carol@example.com");
    let set_cookie = reply.set_cookie().expect("登录必须设置 Cookie");
    assert!(set_cookie.starts_with("session="), "实际：{set_cookie}");
    assert!(
        set_cookie.contains("HttpOnly"),
        "令牌必须对 JavaScript 不可见：{set_cookie}"
    );
    assert!(set_cookie.contains("SameSite=Lax"), "实际：{set_cookie}");
    assert!(set_cookie.contains("Path=/"), "实际：{set_cookie}");
    assert!(
        set_cookie.contains(&format!("Max-Age={SESSION_TTL_SECS}")),
        "实际：{set_cookie}"
    );
    assert!(
        !set_cookie.contains("Secure"),
        "本地 HTTP 配置下不应带 Secure：{set_cookie}"
    );
}

#[tokio::test]
async fn secure_attribute_follows_configuration() {
    let app = test_app(true).await;
    let reply = register_and_login(&app, "dan@example.com").await;
    assert!(reply.set_cookie().unwrap().contains("Secure"));
}

#[tokio::test]
async fn me_requires_a_valid_session() {
    let app = test_app(false).await;
    let logged_in = register_and_login(&app, "erin@example.com").await;
    let cookie = logged_in.cookie_to_send_back().unwrap();

    let authenticated = send(&app.router, get("/auth/me", Some(&cookie))).await;
    assert_eq!(authenticated.status, StatusCode::OK);
    assert_eq!(authenticated.body["user"]["email"], "erin@example.com");

    for (case, cookie) in [
        ("没有 Cookie", None),
        ("伪造的令牌", Some("session=forged-token")),
        ("空令牌", Some("session=")),
        ("超长令牌", Some(&*format!("session={}", "a".repeat(1000)))),
        (
            "名字不对的 Cookie",
            Some(cookie.replace("session=", "other=").as_str()),
        ),
    ] {
        let reply = send(&app.router, get("/auth/me", cookie)).await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{case}");
        assert_eq!(reply.body["error"]["code"], "unauthenticated", "{case}");
    }
}

#[tokio::test]
async fn wrong_password_and_unknown_email_give_the_same_response() {
    let app = test_app(false).await;
    send(
        &app.router,
        json_post(
            "/auth/register",
            &credentials("frank@example.com", "correct-horse"),
            None,
        ),
    )
    .await;

    let wrong_password = send(
        &app.router,
        json_post(
            "/auth/login",
            &credentials("frank@example.com", "wrong-password"),
            None,
        ),
    )
    .await;
    let unknown_email = send(
        &app.router,
        json_post(
            "/auth/login",
            &credentials("nobody@example.com", "correct-horse"),
            None,
        ),
    )
    .await;

    assert_eq!(wrong_password.status, StatusCode::UNAUTHORIZED);
    assert_eq!(unknown_email.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        wrong_password.body, unknown_email.body,
        "不能让攻击者靠响应内容枚举已注册邮箱"
    );
    assert!(wrong_password.set_cookie().is_none() && unknown_email.set_cookie().is_none());
}

#[tokio::test]
async fn logout_revokes_the_session_server_side_and_clears_the_cookie() {
    let app = test_app(false).await;
    let logged_in = register_and_login(&app, "gina@example.com").await;
    let cookie = logged_in.cookie_to_send_back().unwrap();
    assert_eq!(app.session_store.active_session_count(), 1);

    let reply = send(
        &app.router,
        json_post("/auth/logout", &json!({}), Some(&cookie)),
    )
    .await;

    assert_eq!(reply.status, StatusCode::NO_CONTENT);
    let removal = reply.set_cookie().expect("退出必须清除 Cookie");
    assert!(
        removal.starts_with("session=;") || removal.starts_with("session=\"\";"),
        "实际：{removal}"
    );
    assert!(removal.contains("Max-Age=0"), "实际：{removal}");
    assert!(
        removal.contains("Path=/"),
        "删除用的 Path 必须与设置时一致：{removal}"
    );
    assert_eq!(
        app.session_store.active_session_count(),
        0,
        "服务端会话必须真的被删除"
    );

    // 关键：就算浏览器仍然带着旧令牌，也不能再通过认证（不是只靠删 Cookie）。
    let after = send(&app.router, get("/auth/me", Some(&cookie))).await;
    assert_eq!(after.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn logout_without_a_session_is_still_a_success() {
    let app = test_app(false).await;
    let reply = send(&app.router, json_post("/auth/logout", &json!({}), None)).await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn logging_in_again_replaces_the_previous_session() {
    let app = test_app(false).await;
    let first_login = register_and_login(&app, "hana@example.com").await;
    let first_cookie = first_login.cookie_to_send_back().unwrap();

    // 带着旧 Cookie 再次登录：签发新令牌，旧令牌作废（防会话固定）。
    let second_login = send(
        &app.router,
        json_post(
            "/auth/login",
            &credentials("hana@example.com", "correct-horse"),
            Some(&first_cookie),
        ),
    )
    .await;
    let second_cookie = second_login.cookie_to_send_back().unwrap();

    assert_ne!(first_cookie, second_cookie);
    assert_eq!(
        send(&app.router, get("/auth/me", Some(&first_cookie)))
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(&app.router, get("/auth/me", Some(&second_cookie)))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(app.session_store.active_session_count(), 1);
}

#[tokio::test]
async fn write_endpoints_only_accept_json() {
    let app = test_app(false).await;
    let form_post = Request::builder()
        .method("POST")
        .uri("/auth/login")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("email=a%40example.com&password=correct-horse"))
        .unwrap();
    assert_eq!(
        send(&app.router, form_post).await.status,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "这是 CSRF 防护的一部分"
    );

    let no_content_type = Request::builder()
        .method("POST")
        .uri("/auth/login")
        .body(Body::from(
            credentials("a@example.com", "correct-horse").to_string(),
        ))
        .unwrap();
    assert_eq!(
        send(&app.router, no_content_type).await.status,
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
}

#[tokio::test]
async fn malformed_and_oversized_bodies_are_rejected_before_any_work() {
    let app = test_app(false).await;

    let malformed = Request::builder()
        .method("POST")
        .uri("/auth/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{not json"))
        .unwrap();
    assert!(send(&app.router, malformed).await.status.is_client_error());

    let oversized = Request::builder()
        .method("POST")
        .uri("/auth/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({"email": "a@example.com", "password": "x".repeat(64 * 1024)}).to_string(),
        ))
        .unwrap();
    assert_eq!(
        send(&app.router, oversized).await.status,
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

#[tokio::test]
async fn auth_routes_answer_503_when_auth_is_not_configured_but_health_still_works() {
    let router = build_router(AppState {
        health_service: Arc::new(HealthService::new(vec![])),
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
    });

    for request in [
        json_post(
            "/auth/register",
            &credentials("a@example.com", "correct-horse"),
            None,
        ),
        json_post(
            "/auth/login",
            &credentials("a@example.com", "correct-horse"),
            None,
        ),
        json_post("/auth/logout", &json!({}), None),
        get("/auth/me", None),
    ] {
        let reply = send(&router, request).await;
        assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(reply.body["error"]["code"], "auth_unavailable");
    }
    assert_eq!(
        send(&router, get("/live", None)).await.status,
        StatusCode::OK
    );
}

// ---------- 限流 ----------

/// 一个只允许失败 3 次的应用：IP 和账号维度都是 3。
async fn strict_app(trust_forwarded_for: bool) -> TestApp {
    test_app_with(
        false,
        ThrottlePolicy {
            login_max_failures_per_ip: 3,
            login_max_failures_per_account: 3,
            login_window: THROTTLE_WINDOW,
            register_max_per_ip: 2,
            register_window: THROTTLE_WINDOW,
        },
        trust_forwarded_for,
    )
    .await
}

fn login_request(email: &str, password: &str) -> Request<Body> {
    json_post("/auth/login", &credentials(email, password), None)
}

#[tokio::test]
async fn too_many_failed_logins_get_429_with_retry_after_and_even_the_right_password_is_refused() {
    let app = strict_app(false).await;
    send(
        &app.router,
        from_peer(
            json_post(
                "/auth/register",
                &credentials("victim@example.com", "correct-horse"),
                None,
            ),
            "203.0.113.1",
        ),
    )
    .await;

    for attempt in 1..=3 {
        let reply = send(
            &app.router,
            from_peer(
                login_request("victim@example.com", "wrong-password"),
                "198.51.100.7",
            ),
        )
        .await;
        assert_eq!(
            reply.status,
            StatusCode::UNAUTHORIZED,
            "第 {attempt} 次仍是普通的登录失败"
        );
    }

    let blocked = send(
        &app.router,
        from_peer(
            login_request("victim@example.com", "correct-horse"),
            "198.51.100.7",
        ),
    )
    .await;
    assert_eq!(blocked.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(blocked.body["error"]["code"], "too_many_attempts");
    assert_eq!(
        blocked
            .headers
            .get(header::RETRY_AFTER)
            .unwrap()
            .to_str()
            .unwrap(),
        THROTTLE_WINDOW.as_secs().to_string(),
        "Retry-After 必须告诉客户端还要等多少秒"
    );
    assert!(
        blocked.set_cookie().is_none(),
        "被限流时即使密码正确也不能签发会话"
    );
    assert_eq!(app.session_store.active_session_count(), 0);
}

#[tokio::test]
async fn the_throttled_response_does_not_reveal_whether_the_account_exists() {
    let app = strict_app(false).await;
    send(
        &app.router,
        from_peer(
            json_post(
                "/auth/register",
                &credentials("real@example.com", "correct-horse"),
                None,
            ),
            "203.0.113.1",
        ),
    )
    .await;

    // 两个账号各自从不同 IP 失败 3 次，之后各自再试一次。
    let mut final_replies = Vec::new();
    for (email, peers) in [
        (
            "real@example.com",
            [
                "198.51.100.1",
                "198.51.100.2",
                "198.51.100.3",
                "198.51.100.4",
            ],
        ),
        (
            "ghost@example.com",
            [
                "198.51.100.11",
                "198.51.100.12",
                "198.51.100.13",
                "198.51.100.14",
            ],
        ),
    ] {
        let mut last = None;
        for peer in peers {
            last = Some(
                send(
                    &app.router,
                    from_peer(login_request(email, "wrong-password"), peer),
                )
                .await,
            );
        }
        final_replies.push(last.unwrap());
    }

    assert_eq!(final_replies[0].status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(final_replies[0].status, final_replies[1].status);
    assert_eq!(
        final_replies[0].body, final_replies[1].body,
        "存在和不存在的账号，响应必须完全相同"
    );
}

#[tokio::test]
async fn a_spoofed_x_forwarded_for_cannot_evade_the_ip_limit_when_it_is_not_trusted() {
    let app = strict_app(false).await;
    let attacker = "198.51.100.50";

    // 攻击者每次请求都填不同的 X-Forwarded-For，想装成不同的客户端。
    for (index, email) in ["a@example.com", "b@example.com", "c@example.com"]
        .into_iter()
        .enumerate()
    {
        let request = with_header(
            from_peer(login_request(email, "wrong-password"), attacker),
            "x-forwarded-for",
            &format!("10.9.9.{index}"),
        );
        assert_eq!(
            send(&app.router, request).await.status,
            StatusCode::UNAUTHORIZED
        );
    }
    let request = with_header(
        from_peer(login_request("d@example.com", "wrong-password"), attacker),
        "x-forwarded-for",
        "10.9.9.99",
    );

    assert_eq!(
        send(&app.router, request).await.status,
        StatusCode::TOO_MANY_REQUESTS,
        "默认只认 TCP 对端地址"
    );
}

#[tokio::test]
async fn behind_a_trusted_proxy_the_rightmost_forwarded_address_is_used_and_spoofed_entries_are_ignored()
 {
    let app = strict_app(true).await;
    let proxy = "10.0.0.1"; // 反向代理自己的地址：所有请求的 TCP 对端都是它。

    // 客户端 A 在 X-Forwarded-For 最左边伪造各种地址，但最右（代理追加的真实地址）不变。
    for (index, email) in ["a@example.com", "b@example.com", "c@example.com"]
        .into_iter()
        .enumerate()
    {
        let forwarded = format!("1.1.1.{index}, 203.0.113.77");
        let request = with_header(
            from_peer(login_request(email, "wrong-password"), proxy),
            "x-forwarded-for",
            &forwarded,
        );
        assert_eq!(
            send(&app.router, request).await.status,
            StatusCode::UNAUTHORIZED
        );
    }
    let blocked = with_header(
        from_peer(login_request("d@example.com", "wrong-password"), proxy),
        "x-forwarded-for",
        "9.9.9.9, 203.0.113.77",
    );
    assert_eq!(
        send(&app.router, blocked).await.status,
        StatusCode::TOO_MANY_REQUESTS,
        "最左边的伪造值不能让攻击者换到新的额度"
    );

    // 另一个真实客户端（最右地址不同）不受影响，即使它们共用同一个代理。
    let other_client = with_header(
        from_peer(login_request("d@example.com", "wrong-password"), proxy),
        "x-forwarded-for",
        "203.0.113.78",
    );
    assert_eq!(
        send(&app.router, other_client).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn an_unparseable_forwarded_header_falls_back_to_the_tcp_peer() {
    let app = strict_app(true).await;
    // 最右的值不是 IP：说明请求没有经过我们的代理，退回对端地址，而不是去信任左边的条目。
    for email in ["a@example.com", "b@example.com", "c@example.com"] {
        let request = with_header(
            from_peer(login_request(email, "wrong-password"), "198.51.100.60"),
            "x-forwarded-for",
            "203.0.113.1, not-an-ip",
        );
        assert_eq!(
            send(&app.router, request).await.status,
            StatusCode::UNAUTHORIZED
        );
    }
    let request = with_header(
        from_peer(
            login_request("d@example.com", "wrong-password"),
            "198.51.100.60",
        ),
        "x-forwarded-for",
        "203.0.113.1, not-an-ip",
    );
    assert_eq!(
        send(&app.router, request).await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn registration_is_throttled_per_ip_with_429() {
    let app = strict_app(false).await;
    for email in ["one@example.com", "two@example.com"] {
        let request = from_peer(
            json_post("/auth/register", &credentials(email, "correct-horse"), None),
            "198.51.100.70",
        );
        assert_eq!(send(&app.router, request).await.status, StatusCode::CREATED);
    }

    let third = from_peer(
        json_post(
            "/auth/register",
            &credentials("three@example.com", "correct-horse"),
            None,
        ),
        "198.51.100.70",
    );
    let reply = send(&app.router, third).await;
    assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(reply.headers.contains_key(header::RETRY_AFTER));

    let other_ip = from_peer(
        json_post(
            "/auth/register",
            &credentials("three@example.com", "correct-horse"),
            None,
        ),
        "198.51.100.71",
    );
    assert_eq!(
        send(&app.router, other_ip).await.status,
        StatusCode::CREATED,
        "别的 IP 不受影响"
    );
}

#[tokio::test]
async fn requests_without_a_peer_address_still_work_and_are_only_throttled_per_account() {
    // 没有 ConnectInfo（例如进程内测试、或经 unix socket）：拿不到 IP 就只按账号限流，而不是报错。
    let app = strict_app(false).await;
    for _ in 0..3 {
        assert_eq!(
            send(
                &app.router,
                login_request("solo@example.com", "wrong-password")
            )
            .await
            .status,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        send(
            &app.router,
            login_request("solo@example.com", "wrong-password")
        )
        .await
        .status,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        send(
            &app.router,
            login_request("other@example.com", "wrong-password")
        )
        .await
        .status,
        StatusCode::UNAUTHORIZED
    );
}
