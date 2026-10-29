//! 端到端测试：真实的装配代码（`composition::build_app_state`）+ 真实的启动入口（`server::serve`）
//! + 真实的 TCP socket + 真实的 HTTP 客户端。
//!
//! - 第一个测试不需要任何外部服务，默认运行；
//! - 其余测试连接真实的 PostgreSQL 与 Redis，默认跳过，用 `just test real` 显式运行
//!   （连接串来自 `TEST_DATABASE_URL`、`TEST_REDIS_URL`，缺失会失败并提示，而不是静默跳过）。
//!
//! 限流计数按 IP 存放，而这些测试的请求全都来自 127.0.0.1，所以**每个真实服务测试使用自己独立的
//! Redis 库**（编号见下面的常量），开始和结束时清空它。这样测试之间、测试与多次运行之间互不污染，
//! 也不会碰到默认的 0 号库。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tpl_accounts::composition::build_app_state;
use tpl_infrastructure::config::{DatabaseSettings, RedisSettings, Settings};

/// 完整认证流程测试使用的 Redis 库。
const FULL_FLOW_REDIS_DB: u8 = 8;
/// 限流测试使用的 Redis 库。
const THROTTLE_REDIS_DB: u8 = 9;
/// 管理后台测试使用的 Redis 库。
const ADMIN_REDIS_DB: u8 = 10;

struct RunningServer {
    base_url: String,
}

impl RunningServer {
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }
}

/// 用给定配置启动一个真实的服务，监听本机的随机空闲端口。启动方式与 `main` 完全相同。
async fn start_server(settings: &Settings) -> RunningServer {
    let app_state = build_app_state(settings).await.expect("装配应当成功");
    let router = tpl_api::build_router(app_state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        tpl_accounts::server::serve(listener, router, std::future::pending())
            .await
            .unwrap();
    });
    RunningServer {
        base_url: format!("http://{address}"),
    }
}

/// 模拟浏览器：自动保存并回传 Cookie。
fn browser() -> Client {
    Client::builder().cookie_store(true).build().unwrap()
}

/// `TEST_REDIS_URL` 换成指定编号的库：末尾已有 `/<数字>` 就替换它，否则追加 `/<库号>`。
fn redis_url_for_db(db: u8) -> String {
    let base_url = std::env::var("TEST_REDIS_URL").expect("请设置 TEST_REDIS_URL");
    match base_url.rsplit_once('/') {
        Some((prefix, last_segment))
            if !last_segment.is_empty() && last_segment.chars().all(|c| c.is_ascii_digit()) =>
        {
            format!("{prefix}/{db}")
        }
        _ => format!("{}/{db}", base_url.trim_end_matches('/')),
    }
}

/// 清空指定编号的 Redis 库（只清测试自己的库）。
async fn flush_redis_db(db: u8) {
    let client = redis::Client::open(redis_url_for_db(db)).unwrap();
    let mut connection = client.get_multiplexed_async_connection().await.unwrap();
    redis::cmd("FLUSHDB")
        .query_async::<()>(&mut connection)
        .await
        .unwrap();
}

fn real_services_settings(redis_db: u8) -> Settings {
    Settings {
        database: DatabaseSettings {
            url: Some(std::env::var("TEST_DATABASE_URL").expect("请设置 TEST_DATABASE_URL")),
            ..DatabaseSettings::default()
        },
        redis: RedisSettings {
            url: Some(redis_url_for_db(redis_db)),
            ..RedisSettings::default()
        },
        ..Settings::default()
    }
}

async fn json_of(response: reqwest::Response) -> Value {
    response.json().await.unwrap_or(Value::Null)
}

fn unique_suffix() -> String {
    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
        .to_string();
    format!(
        "{timestamp}-{}-{}",
        std::process::id(),
        NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
    )
}

#[tokio::test]
async fn probes_work_over_a_real_socket_without_any_external_service() {
    let server = start_server(&Settings::default()).await;
    let client = browser();

    let live = client.get(server.url("/live")).send().await.unwrap();
    assert_eq!(live.status(), StatusCode::OK);
    assert_eq!(json_of(live).await, json!({"status": "ok"}));

    let ready = client.get(server.url("/ready")).send().await.unwrap();
    assert_eq!(ready.status(), StatusCode::OK);
    assert_eq!(
        json_of(ready).await,
        json!({"status": "ready", "checks": {}})
    );

    // 没配置数据库和 Redis：认证未启用，但进程照常提供服务。
    let me = client.get(server.url("/auth/me")).send().await.unwrap();
    assert_eq!(me.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json_of(me).await["error"]["code"], "auth_unavailable");
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL 与 Redis，设置 TEST_DATABASE_URL、TEST_REDIS_URL 后用 `just test real` 运行"]
async fn full_auth_flow_against_real_postgres_and_redis() {
    flush_redis_db(FULL_FLOW_REDIS_DB).await;
    let settings = real_services_settings(FULL_FLOW_REDIS_DB);
    let server = start_server(&settings).await;
    let database_pool = sqlx::PgPool::connect(settings.database.url.as_deref().unwrap())
        .await
        .unwrap();

    // 清理历史残留（上次测试中途失败时可能留下），再用本次唯一的邮箱。
    sqlx::query("DELETE FROM users WHERE email LIKE 'e2e-%@example.test'")
        .execute(&database_pool)
        .await
        .unwrap();
    let email = format!("e2e-{}@example.test", unique_suffix());
    let password = "correct-horse-battery";

    let client = browser();

    // 就绪探针同时覆盖两个真实依赖。
    let ready = client.get(server.url("/ready")).send().await.unwrap();
    assert_eq!(ready.status(), StatusCode::OK);
    assert_eq!(
        json_of(ready).await,
        json!({"status": "ready", "checks": {"postgres": "ok", "redis": "ok"}})
    );

    // 注册：201，且不自动登录。
    let registered = client
        .post(server.url("/auth/register"))
        .json(&json!({"email": email.to_uppercase(), "password": password}))
        .send()
        .await
        .unwrap();
    assert_eq!(registered.status(), StatusCode::CREATED);
    assert_eq!(
        json_of(registered).await["user"]["email"],
        email,
        "邮箱应被规范化为小写"
    );
    let before_login = client.get(server.url("/auth/me")).send().await.unwrap();
    assert_eq!(before_login.status(), StatusCode::UNAUTHORIZED);

    // 重复注册：409。
    let duplicate = client
        .post(server.url("/auth/register"))
        .json(&json!({"email": email, "password": "another-password"}))
        .send()
        .await
        .unwrap();
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);

    // 数据库里存的是 argon2id 哈希，而不是明文。
    let stored_hash: String =
        sqlx::query_scalar("SELECT password_hash FROM users WHERE email = $1")
            .bind(&email)
            .fetch_one(&database_pool)
            .await
            .unwrap();
    assert!(stored_hash.starts_with("$argon2id$"), "实际：{stored_hash}");
    assert!(!stored_hash.contains(password));

    // 密码错误：401。
    let wrong_password = client
        .post(server.url("/auth/login"))
        .json(&json!({"email": email, "password": "wrong-password"}))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong_password.status(), StatusCode::UNAUTHORIZED);

    // 登录：200，Cookie 属性在真实 HTTP 上也要正确。
    let login = client
        .post(server.url("/auth/login"))
        .json(&json!({"email": email, "password": password}))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let session_cookie = login
        .cookies()
        .find(|cookie| cookie.name() == "session")
        .expect("登录必须设置 session Cookie");
    assert!(session_cookie.http_only());
    assert!(session_cookie.same_site_lax());
    assert_eq!(session_cookie.path(), Some("/"));
    assert_eq!(
        session_cookie.max_age().map(|age| age.as_secs()),
        Some(7 * 24 * 60 * 60)
    );
    let session_token = session_cookie.value().to_string();
    assert_eq!(session_token.len(), 64, "256 位随机令牌的十六进制");

    // 已登录：/auth/me 通过浏览器自动回传的 Cookie 拿到当前用户。
    let me = client.get(server.url("/auth/me")).send().await.unwrap();
    assert_eq!(me.status(), StatusCode::OK);
    assert_eq!(json_of(me).await["user"]["email"], email);

    // 另一个没有 Cookie 的客户端：未登录。
    let stranger = reqwest::get(server.url("/auth/me")).await.unwrap();
    assert_eq!(stranger.status(), StatusCode::UNAUTHORIZED);

    // 退出：204；之后同一个浏览器不再能通过认证。
    let logout = client
        .post(server.url("/auth/logout"))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    let after_logout = client.get(server.url("/auth/me")).send().await.unwrap();
    assert_eq!(after_logout.status(), StatusCode::UNAUTHORIZED);

    // 关键：用退出前抓到的令牌手动重放，也必须被拒绝——证明会话是在服务端（Redis）真的被删除，
    // 而不是只靠浏览器删 Cookie。
    let replay = reqwest::Client::new()
        .get(server.url("/auth/me"))
        .header("Cookie", format!("session={session_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);

    sqlx::query("DELETE FROM users WHERE email = $1")
        .bind(&email)
        .execute(&database_pool)
        .await
        .unwrap();
    flush_redis_db(FULL_FLOW_REDIS_DB).await;
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL 与 Redis，设置 TEST_DATABASE_URL、TEST_REDIS_URL 后用 `just test real` 运行"]
async fn login_throttling_uses_the_real_tcp_peer_address_and_ignores_a_spoofed_forwarded_header() {
    flush_redis_db(THROTTLE_REDIS_DB).await;
    let mut settings = real_services_settings(THROTTLE_REDIS_DB);
    settings.auth.login_max_failures_per_ip = 3;
    settings.auth.login_max_failures_per_account = 1000; // 确保拦住它的是「IP」维度
    settings.auth.trust_forwarded_for = false;
    let server = start_server(&settings).await;
    let client = reqwest::Client::new();

    let attempt = |email: String, forwarded_for: &'static str| {
        let request = client
            .post(server.url("/auth/login"))
            .header("X-Forwarded-For", forwarded_for)
            .json(&json!({"email": email, "password": "wrong-password"}));
        async move { request.send().await.unwrap() }
    };

    // 前 3 次是普通的登录失败；每次都换邮箱、换伪造的 X-Forwarded-For，模拟撞库加伪造来源。
    for (index, forwarded_for) in ["10.1.1.1", "10.1.1.2", "10.1.1.3"].into_iter().enumerate() {
        let reply = attempt(format!("throttle-{index}@example.test"), forwarded_for).await;
        assert_eq!(reply.status(), StatusCode::UNAUTHORIZED);
    }

    // 第 4 次：被限流。这证明服务拿到了真实的 TCP 对端地址（走的是 `server::serve`），
    // 且不信任客户端自己写的 X-Forwarded-For。
    let blocked = attempt("throttle-4@example.test".to_string(), "10.1.1.4").await;
    assert_eq!(blocked.status(), StatusCode::TOO_MANY_REQUESTS);
    let retry_after: u64 = blocked
        .headers()
        .get("retry-after")
        .expect("429 必须带 Retry-After")
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=15 * 60).contains(&retry_after), "实际：{retry_after}");
    assert_eq!(json_of(blocked).await["error"]["code"], "too_many_attempts");

    // Redis 里的限流键不含邮箱或 IP 明文。
    let redis_client = redis::Client::open(redis_url_for_db(THROTTLE_REDIS_DB)).unwrap();
    let mut redis_connection = redis_client
        .get_multiplexed_async_connection()
        .await
        .unwrap();
    let stored_keys: Vec<String> = redis::cmd("KEYS")
        .arg("throttle:*")
        .query_async(&mut redis_connection)
        .await
        .unwrap();
    assert!(!stored_keys.is_empty(), "限流计数应该真的写进了 Redis");
    assert!(
        stored_keys
            .iter()
            .all(|key| !key.contains("example") && !key.contains("127.0.0.1")),
        "键里不应有明文：{stored_keys:?}"
    );

    flush_redis_db(THROTTLE_REDIS_DB).await;
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL 与 Redis，设置 TEST_DATABASE_URL、TEST_REDIS_URL 后用 `just test real` 运行"]
async fn admin_console_flow_against_real_postgres_and_redis() {
    flush_redis_db(ADMIN_REDIS_DB).await;
    let suffix = unique_suffix();
    let admin_username = format!("e2e-admin-{suffix}");
    let admin_password = "admin-correct-horse";
    let mut settings = real_services_settings(ADMIN_REDIS_DB);
    settings.admin.bootstrap_username = Some(admin_username.clone());
    settings.admin.bootstrap_password = Some(admin_password.to_string());
    let server = start_server(&settings).await;
    let database_pool = sqlx::PgPool::connect(settings.database.url.as_deref().unwrap())
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE email LIKE 'e2e-console-%@example.test'")
        .execute(&database_pool)
        .await
        .unwrap();

    // 引导账号：库里是 argon2id 哈希；没有注册入口，也没有任何接口能创建管理员。
    let stored_hash: String =
        sqlx::query_scalar("SELECT password_hash FROM admin_users WHERE username = $1")
            .bind(&admin_username)
            .fetch_one(&database_pool)
            .await
            .unwrap();
    assert!(stored_hash.starts_with("$argon2id$"), "实际：{stored_hash}");
    for path in ["/admin/auth/register", "/admin/users/register"] {
        let attempt = reqwest::Client::new()
            .post(server.url(path))
            .json(&json!({"username": "evil", "password": "evil-password"}))
            .send()
            .await
            .unwrap();
        assert!(
            [StatusCode::NOT_FOUND, StatusCode::METHOD_NOT_ALLOWED].contains(&attempt.status()),
            "{path} 不应存在：{}",
            attempt.status()
        );
    }

    // 再次启动（模拟重启/多实例）并带着另一个密码：不能覆盖已有管理员的密码。
    let mut restart_settings = settings.clone();
    restart_settings.admin.bootstrap_password = Some("a-different-password".to_string());
    let restarted = start_server(&restart_settings).await;
    let login_request = |server: &RunningServer, password: &str| {
        reqwest::Client::new()
            .post(server.url("/admin/auth/login"))
            .json(&json!({"identifier": admin_username, "password": password}))
    };
    assert_eq!(
        login_request(&restarted, "a-different-password")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );

    // 管理员登录：Cookie 属性正确，会话存在管理员自己的 Redis 命名空间里。
    let admin_browser = browser();
    let login = admin_browser
        .post(server.url("/admin/auth/login"))
        .json(&json!({"identifier": admin_username.to_uppercase(), "password": admin_password}))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let admin_cookie = login
        .cookies()
        .find(|cookie| cookie.name() == "admin_session")
        .expect("登录必须设置 admin_session Cookie");
    assert!(admin_cookie.http_only() && admin_cookie.same_site_lax());
    assert_eq!(
        admin_cookie.max_age().map(|age| age.as_secs()),
        Some(12 * 60 * 60)
    );
    let admin_token = admin_cookie.value().to_string();
    assert_eq!(json_of(login).await["username"], admin_username);

    let redis_client = redis::Client::open(redis_url_for_db(ADMIN_REDIS_DB)).unwrap();
    let mut redis_connection = redis_client
        .get_multiplexed_async_connection()
        .await
        .unwrap();
    let admin_keys: Vec<String> = redis::cmd("KEYS")
        .arg("admin_session:*")
        .query_async(&mut redis_connection)
        .await
        .unwrap();
    let user_keys: Vec<String> = redis::cmd("KEYS")
        .arg("session:*")
        .query_async(&mut redis_connection)
        .await
        .unwrap();
    assert_eq!((admin_keys.len(), user_keys.len()), (1, 0));
    assert!(admin_keys.iter().all(|key| !key.contains(&admin_token)));

    // 普通用户注册并登录；他的会话进不了后台。
    let email = format!("e2e-console-{suffix}@example.test");
    let user_browser = browser();
    let registered = user_browser
        .post(server.url("/auth/register"))
        .json(&json!({"email": email, "password": "correct-horse-battery"}))
        .send()
        .await
        .unwrap();
    assert_eq!(registered.status(), StatusCode::CREATED);
    let user_id = json_of(registered).await["user"]["id"].as_i64().unwrap();
    let user_login = |client: &Client| {
        client
            .post(server.url("/auth/login"))
            .json(&json!({"email": email, "password": "correct-horse-battery"}))
            .send()
    };
    let login = user_login(&user_browser).await.unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let user_token = login
        .cookies()
        .find(|cookie| cookie.name() == "session")
        .unwrap()
        .value()
        .to_string();
    for cookie in [
        format!("session={user_token}"),
        format!("admin_session={user_token}"),
    ] {
        let reply = reqwest::Client::new()
            .get(server.url("/admin/users"))
            .header("Cookie", cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(reply.status(), StatusCode::UNAUTHORIZED);
    }
    let admin_as_user = reqwest::Client::new()
        .get(server.url("/auth/me"))
        .header("Cookie", format!("session={admin_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(admin_as_user.status(), StatusCode::UNAUTHORIZED);

    // 后台能搜到这个用户（真实 PostgreSQL 的过滤与分页）。
    let listed = admin_browser
        .get(server.url("/admin/users?keyword=E2E-CONSOLE&page_size=5"))
        .send()
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let listed = json_of(listed).await;
    assert_eq!(listed["total"], 1);
    assert_eq!(listed["items"][0]["email"], email);
    assert_eq!(listed["items"][0]["status"], "active");
    assert_eq!(listed["items"][0]["id"], user_id.to_string());

    // 停用：立即生效，不需要等会话过期。
    let disabled = admin_browser
        .post(server.url(&format!("/admin/users/{user_id}/disable")))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(disabled.status(), StatusCode::OK);
    assert_eq!(json_of(disabled).await["status"], "disabled");
    let me = user_browser
        .get(server.url("/auth/me"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        me.status(),
        StatusCode::UNAUTHORIZED,
        "已有登录态应立即失效"
    );
    let blocked = user_login(&browser()).await.unwrap();
    assert_eq!(blocked.status(), StatusCode::FORBIDDEN);
    assert_eq!(json_of(blocked).await["error"]["code"], "account_disabled");
    let status_in_db: String = sqlx::query_scalar("SELECT status FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&database_pool)
        .await
        .unwrap();
    assert_eq!(status_in_db, "disabled");

    // 重新启用：账号数据没有丢，旧会话与新登录都恢复。
    let enabled = admin_browser
        .post(server.url(&format!("/admin/users/{user_id}/enable")))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(json_of(enabled).await["status"], "active");
    assert_eq!(
        user_browser
            .get(server.url("/auth/me"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        user_login(&browser()).await.unwrap().status(),
        StatusCode::OK
    );

    // 管理员退出后，旧令牌在服务端（Redis）真的失效。
    let logout = admin_browser
        .post(server.url("/admin/auth/logout"))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    let replay = reqwest::Client::new()
        .get(server.url("/admin/users"))
        .header("Cookie", format!("admin_session={admin_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);

    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user_id)
        .execute(&database_pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM admin_users WHERE username = $1")
        .bind(&admin_username)
        .execute(&database_pool)
        .await
        .unwrap();
    flush_redis_db(ADMIN_REDIS_DB).await;
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL 与 Redis，设置 TEST_DATABASE_URL、TEST_REDIS_URL 后用 `just test real` 运行"]
async fn admin_login_throttling_is_stricter_and_separate_from_user_login() {
    const THROTTLE_ADMIN_REDIS_DB: u8 = 11;
    flush_redis_db(THROTTLE_ADMIN_REDIS_DB).await;
    let suffix = unique_suffix();
    let admin_username = format!("e2e-admin-{suffix}");
    let mut settings = real_services_settings(THROTTLE_ADMIN_REDIS_DB);
    settings.admin.bootstrap_username = Some(admin_username.clone());
    settings.admin.bootstrap_password = Some("admin-correct-horse".to_string());
    settings.admin.login_max_failures_per_account = 3;
    settings.auth.login_max_failures_per_account = 3;
    let server = start_server(&settings).await;
    let client = reqwest::Client::new();
    let admin_login = |password: &'static str| {
        client
            .post(server.url("/admin/auth/login"))
            .json(&json!({"identifier": admin_username, "password": password}))
            .send()
    };

    for _ in 0..3 {
        assert_eq!(
            admin_login("wrong-wrong").await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let blocked = admin_login("admin-correct-horse").await.unwrap();
    assert_eq!(blocked.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(blocked.headers().contains_key("retry-after"));

    // 同名的普通账号登录失败计数是另一套：管理员被锁，不影响它，反之亦然。
    let user_attempt = client
        .post(server.url("/auth/login"))
        .json(
            &json!({"email": format!("{admin_username}@example.test"), "password": "x-x-x-x-x-x"}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(user_attempt.status(), StatusCode::UNAUTHORIZED);

    let database_pool = sqlx::PgPool::connect(settings.database.url.as_deref().unwrap())
        .await
        .unwrap();
    sqlx::query("DELETE FROM admin_users WHERE username = $1")
        .bind(&admin_username)
        .execute(&database_pool)
        .await
        .unwrap();
    flush_redis_db(THROTTLE_ADMIN_REDIS_DB).await;
}
