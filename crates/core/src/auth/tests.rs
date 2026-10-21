//! `auth` 的测试：用内存替身验证规则，不连任何外部服务。

use std::sync::atomic::{AtomicUsize, Ordering};

use super::testing::{
    InMemoryAttemptCounter, InMemorySessionStore, InMemoryUserRepository, PlainPasswordHasher,
};
use super::*;

const SESSION_TTL: Duration = Duration::from_secs(3600);
const WINDOW: Duration = Duration::from_secs(15 * 60);

/// 统计哈希与校验被调用的次数，用来证明「被限流时没有进入哈希」。
#[derive(Default)]
struct CountingHasher {
    hash_calls: AtomicUsize,
    verify_calls: AtomicUsize,
}

#[async_trait]
impl PasswordHasher for CountingHasher {
    async fn hash(&self, password: &str) -> Result<String, PortFailure> {
        self.hash_calls.fetch_add(1, Ordering::SeqCst);
        PlainPasswordHasher.hash(password).await
    }

    async fn verify(&self, password: &str, stored_hash: &str) -> Result<bool, PortFailure> {
        self.verify_calls.fetch_add(1, Ordering::SeqCst);
        PlainPasswordHasher.verify(password, stored_hash).await
    }
}

struct Fixture {
    service: AuthService,
    user_repository: Arc<InMemoryUserRepository>,
    session_store: Arc<InMemorySessionStore>,
    attempt_counter: Arc<InMemoryAttemptCounter>,
    hasher: Arc<CountingHasher>,
}

fn policy(login_per_ip: u32, login_per_account: u32, register_per_ip: u32) -> ThrottlePolicy {
    ThrottlePolicy {
        login_max_failures_per_ip: login_per_ip,
        login_max_failures_per_account: login_per_account,
        login_window: WINDOW,
        register_max_per_ip: register_per_ip,
        register_window: WINDOW * 4,
    }
}

async fn fixture_with(throttle: ThrottlePolicy) -> Fixture {
    let session_store = Arc::new(InMemorySessionStore::default());
    let attempt_counter = Arc::new(InMemoryAttemptCounter::default());
    let hasher = Arc::new(CountingHasher::default());
    let user_repository = Arc::new(InMemoryUserRepository::default());
    let service = AuthService::new(
        AuthPorts {
            user_repository: user_repository.clone(),
            session_store: session_store.clone(),
            password_hasher: hasher.clone(),
            attempt_counter: attempt_counter.clone(),
        },
        AuthPolicy {
            session_ttl: SESSION_TTL,
            throttle,
        },
    )
    .await
    .unwrap();
    Fixture {
        service,
        user_repository,
        session_store,
        attempt_counter,
        hasher,
    }
}

/// 限制宽松的夹具：让与限流无关的测试不会被限流干扰。
async fn fixture() -> Fixture {
    fixture_with(policy(1000, 1000, 1000)).await
}

fn ip(text: &str) -> Option<IpAddr> {
    Some(text.parse().unwrap())
}

async fn register(fixture: &Fixture, email: &str) {
    fixture
        .service
        .register(email, "correct-horse", None)
        .await
        .unwrap();
}

async fn failed_login(fixture: &Fixture, email: &str, client_ip: Option<IpAddr>) -> AuthError {
    fixture
        .service
        .login(email, "wrong-password", client_ip)
        .await
        .unwrap_err()
}

// ---------- 校验规则 ----------

#[test]
fn email_is_trimmed_and_lowercased() {
    assert_eq!(
        normalize_email("  Alice@Example.COM ").as_deref(),
        Ok("alice@example.com")
    );
}

#[test]
fn implausible_emails_are_rejected() {
    for bad_email in [
        "",
        "no-at-sign",
        "@example.com",
        "alice@",
        "alice@nodot",
        "alice@.com",
        "alice@example.",
        "a@b@example.com",
        "ali ce@example.com",
        "alice@exa\nmple.com",
    ] {
        assert_eq!(
            normalize_email(bad_email),
            Err(AuthError::InvalidEmail),
            "应被拒绝：{bad_email:?}"
        );
    }
    let too_long = format!("{}@example.com", "a".repeat(260));
    assert_eq!(normalize_email(&too_long), Err(AuthError::InvalidEmail));
}

#[test]
fn password_length_rules_count_chars_for_min_and_bytes_for_max() {
    assert_eq!(
        validate_password("1234567"),
        Err(AuthError::PasswordTooShort)
    );
    assert_eq!(validate_password("12345678"), Ok(()));
    // 8 个汉字 = 24 字节，按字符数算够长，按字节数算也没超上限。
    assert_eq!(validate_password("密码密码密码密码"), Ok(()));
    // 7 个汉字不够 8 个字符，即使字节数（21）超过 8。
    assert_eq!(
        validate_password("密码密码密码密"),
        Err(AuthError::PasswordTooShort)
    );
    assert_eq!(
        validate_password(&"x".repeat(MAX_PASSWORD_BYTES + 1)),
        Err(AuthError::PasswordTooLong)
    );
    assert_eq!(validate_password(&"x".repeat(MAX_PASSWORD_BYTES)), Ok(()));
}

// ---------- 基本流程 ----------

#[tokio::test]
async fn register_then_login_then_authenticate_then_logout() {
    let Fixture { service, .. } = fixture().await;

    let registered = service
        .register(" Alice@Example.com ", "correct-horse", None)
        .await
        .unwrap();
    assert_eq!(registered.email, "alice@example.com");

    let (logged_in, token) = service
        .login("ALICE@example.com", "correct-horse", None)
        .await
        .unwrap();
    assert_eq!(logged_in, registered);

    assert_eq!(service.authenticate(&token).await, Ok(registered));

    service.logout(&token).await.unwrap();
    assert_eq!(
        service.authenticate(&token).await,
        Err(AuthError::Unauthenticated)
    );
    // 重复退出是幂等的。
    assert_eq!(service.logout(&token).await, Ok(()));
}

#[tokio::test]
async fn registering_the_same_email_twice_is_a_conflict_even_with_different_case() {
    let fixture = fixture().await;
    register(&fixture, "bob@example.com").await;
    assert_eq!(
        fixture
            .service
            .register("BOB@example.com", "another-password", None)
            .await,
        Err(AuthError::EmailTaken)
    );
}

#[tokio::test]
async fn register_validates_before_touching_storage() {
    let Fixture { service, .. } = fixture().await;
    assert_eq!(
        service
            .register("not-an-email", "correct-horse", None)
            .await,
        Err(AuthError::InvalidEmail)
    );
    assert_eq!(
        service.register("c@example.com", "short", None).await,
        Err(AuthError::PasswordTooShort)
    );
}

#[tokio::test]
async fn unknown_email_and_wrong_password_are_indistinguishable() {
    let fixture = fixture().await;
    register(&fixture, "dave@example.com").await;
    let service = &fixture.service;

    let wrong_password = service
        .login("dave@example.com", "wrong-password", None)
        .await;
    let unknown_email = service
        .login("nobody@example.com", "correct-horse", None)
        .await;
    let malformed_email = service.login("not-an-email", "correct-horse", None).await;
    let oversized_password = service
        .login("dave@example.com", &"x".repeat(1000), None)
        .await;

    for outcome in [
        wrong_password,
        unknown_email,
        malformed_email,
        oversized_password,
    ] {
        assert_eq!(outcome.map(|_| ()), Err(AuthError::InvalidCredentials));
    }
    assert_eq!(
        fixture.session_store.active_session_count(),
        0,
        "失败的登录不能产生会话"
    );
}

#[tokio::test]
async fn logging_in_twice_creates_independent_sessions() {
    let fixture = fixture().await;
    register(&fixture, "erin@example.com").await;
    let service = &fixture.service;
    let (_, first_token) = service
        .login("erin@example.com", "correct-horse", None)
        .await
        .unwrap();
    let (_, second_token) = service
        .login("erin@example.com", "correct-horse", None)
        .await
        .unwrap();

    service.logout(&first_token).await.unwrap();

    assert_eq!(
        service.authenticate(&first_token).await,
        Err(AuthError::Unauthenticated)
    );
    assert!(
        service.authenticate(&second_token).await.is_ok(),
        "退出一个设备不应影响另一个"
    );
}

#[test]
fn secrets_never_appear_in_debug_output() {
    let credentials = StoredCredentials {
        user: User {
            id: UserId(1),
            email: "a@example.com".to_string(),
            status: UserStatus::Active,
            created_at: "2026-01-01T00:00:00Z".to_string(),
        },
        password_hash: "$argon2id$secret-hash".to_string(),
    };
    assert!(!format!("{credentials:?}").contains("secret-hash"));
    assert!(!format!("{:?}", SessionToken::new("super-secret-token")).contains("super-secret"));
}

// ---------- 账号停用 ----------

async fn set_status(fixture: &Fixture, email: &str, status: UserStatus) {
    let user_id = fixture
        .user_repository
        .find_credentials_by_email(email)
        .await
        .unwrap()
        .unwrap()
        .user
        .id;
    fixture
        .user_repository
        .set_status(user_id, status)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn disabling_a_user_invalidates_an_existing_session_on_the_very_next_check() {
    let fixture = fixture().await;
    register(&fixture, "frank@example.com").await;
    let (_, token) = fixture
        .service
        .login("frank@example.com", "correct-horse", None)
        .await
        .unwrap();
    assert!(fixture.service.authenticate(&token).await.is_ok());

    set_status(&fixture, "frank@example.com", UserStatus::Disabled).await;

    assert_eq!(
        fixture.service.authenticate(&token).await,
        Err(AuthError::Unauthenticated),
        "停用必须立即生效，不能等会话自然过期"
    );

    // 重新启用后同一个会话又可用：停用没有销毁任何数据。
    set_status(&fixture, "frank@example.com", UserStatus::Active).await;
    assert!(fixture.service.authenticate(&token).await.is_ok());
}

#[tokio::test]
async fn a_disabled_user_cannot_log_in_but_only_learns_that_after_the_right_password() {
    let fixture = fixture().await;
    register(&fixture, "gina@example.com").await;
    set_status(&fixture, "gina@example.com", UserStatus::Disabled).await;

    let with_right_password = fixture
        .service
        .login("gina@example.com", "correct-horse", None)
        .await;
    let with_wrong_password = fixture
        .service
        .login("gina@example.com", "wrong-password", None)
        .await;

    assert_eq!(
        with_right_password.map(|_| ()),
        Err(AuthError::AccountDisabled)
    );
    assert_eq!(
        with_wrong_password.map(|_| ()),
        Err(AuthError::InvalidCredentials),
        "密码不对时不能透露账号已停用，否则就成了探测邮箱是否存在的手段"
    );
    assert_eq!(
        fixture.session_store.active_session_count(),
        0,
        "停用的账号不能签发会话"
    );
}

#[test]
fn user_status_round_trips_and_rejects_unknown_values() {
    for status in [UserStatus::Active, UserStatus::Disabled] {
        assert_eq!(UserStatus::parse(status.as_str()), Some(status));
    }
    assert_eq!(
        UserStatus::parse("ACTIVE"),
        None,
        "取值区分大小写，不认识就是不认识"
    );
    assert_eq!(UserStatus::parse(""), None);
}

// ---------- 限流 ----------

#[tokio::test]
async fn an_account_is_throttled_across_ips_and_even_the_right_password_is_refused() {
    // 按账号 3 次；按 IP 放得很宽，确保拦住它的是「账号」维度。
    let fixture = fixture_with(policy(1000, 3, 1000)).await;
    register(&fixture, "frank@example.com").await;
    for attacker_ip in ["198.51.100.1", "198.51.100.2", "198.51.100.3"] {
        assert_eq!(
            failed_login(&fixture, "frank@example.com", ip(attacker_ip)).await,
            AuthError::InvalidCredentials
        );
    }
    let verify_calls_before = fixture.hasher.verify_calls.load(Ordering::SeqCst);

    // 第 4 次来自全新的 IP，并且密码是对的：仍被拒绝，而且没有进入哈希校验。
    let blocked = fixture
        .service
        .login("frank@example.com", "correct-horse", ip("198.51.100.99"))
        .await;

    assert_eq!(
        blocked.map(|_| ()),
        Err(AuthError::TooManyAttempts {
            retry_after: WINDOW
        })
    );
    assert_eq!(
        fixture.hasher.verify_calls.load(Ordering::SeqCst),
        verify_calls_before
    );
    assert_eq!(fixture.session_store.active_session_count(), 0);
}

#[tokio::test]
async fn an_ip_is_throttled_across_accounts_but_other_ips_are_unaffected() {
    let fixture = fixture_with(policy(3, 1000, 1000)).await;
    register(&fixture, "gina@example.com").await;
    let attacker = ip("203.0.113.5");
    // 撞库：同一个 IP 依次试不同的邮箱。
    for email in ["a@example.com", "b@example.com", "c@example.com"] {
        assert_eq!(
            failed_login(&fixture, email, attacker).await,
            AuthError::InvalidCredentials
        );
    }

    assert!(matches!(
        failed_login(&fixture, "d@example.com", attacker).await,
        AuthError::TooManyAttempts { .. }
    ));
    // 同一个 IP 上，连真实账号的正确密码也被挡住。
    assert!(matches!(
        fixture
            .service
            .login("gina@example.com", "correct-horse", attacker)
            .await,
        Err(AuthError::TooManyAttempts { .. })
    ));
    // 别的 IP 不受影响。
    assert!(
        fixture
            .service
            .login("gina@example.com", "correct-horse", ip("203.0.113.6"))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn unknown_emails_are_counted_so_throttling_does_not_reveal_which_accounts_exist() {
    let fixture = fixture_with(policy(1000, 3, 1000)).await;
    register(&fixture, "real@example.com").await;

    let mut real_outcomes = Vec::new();
    let mut ghost_outcomes = Vec::new();
    for _ in 0..5 {
        real_outcomes.push(failed_login(&fixture, "real@example.com", None).await);
        ghost_outcomes.push(failed_login(&fixture, "ghost@example.com", None).await);
    }

    assert_eq!(
        real_outcomes, ghost_outcomes,
        "存在与不存在的账号，被限流的节奏必须完全一致"
    );
    assert!(matches!(
        real_outcomes[3],
        AuthError::TooManyAttempts { .. }
    ));
}

#[tokio::test]
async fn success_clears_the_account_counter_but_not_the_ip_counter() {
    let fixture = fixture_with(policy(10, 10, 1000)).await;
    register(&fixture, "hana@example.com").await;
    let client = ip("203.0.113.7");
    failed_login(&fixture, "hana@example.com", client).await;
    failed_login(&fixture, "hana@example.com", client).await;

    fixture
        .service
        .login("hana@example.com", "correct-horse", client)
        .await
        .unwrap();

    let counter = &fixture.attempt_counter;
    assert_eq!(
        counter.status("login:account:hana@example.com").await,
        Ok(None)
    );
    assert_eq!(
        counter
            .status("login:ip:203.0.113.7")
            .await
            .unwrap()
            .map(|status| status.count),
        Some(2),
        "IP 计数不能被登录成功清零，否则攻击者用自己的账号登录一次就能重置它"
    );
}

#[tokio::test]
async fn an_attacker_cannot_dodge_the_ip_limit_by_logging_into_their_own_account() {
    let fixture = fixture_with(policy(3, 1000, 1000)).await;
    register(&fixture, "mallory@example.com").await;
    let attacker = ip("203.0.113.9");

    failed_login(&fixture, "victim1@example.com", attacker).await;
    failed_login(&fixture, "victim2@example.com", attacker).await;
    fixture
        .service
        .login("mallory@example.com", "correct-horse", attacker)
        .await
        .unwrap();
    failed_login(&fixture, "victim3@example.com", attacker).await;

    assert!(matches!(
        failed_login(&fixture, "victim4@example.com", attacker).await,
        AuthError::TooManyAttempts { .. }
    ));
}

#[tokio::test]
async fn missing_ip_falls_back_to_account_limit_and_bad_email_falls_back_to_ip_limit() {
    let fixture = fixture_with(policy(2, 2, 1000)).await;
    // 拿不到 IP：只按账号限流。
    failed_login(&fixture, "ivan@example.com", None).await;
    failed_login(&fixture, "ivan@example.com", None).await;
    assert!(matches!(
        failed_login(&fixture, "ivan@example.com", None).await,
        AuthError::TooManyAttempts { .. }
    ));

    // 邮箱格式不合法：只按 IP 限流。
    let client = ip("203.0.113.10");
    failed_login(&fixture, "not-an-email", client).await;
    failed_login(&fixture, "also not an email", client).await;
    assert!(matches!(
        failed_login(&fixture, "still-bad", client).await,
        AuthError::TooManyAttempts { .. }
    ));
}

#[tokio::test]
async fn ipv6_addresses_in_one_slash_64_share_a_limit() {
    let fixture = fixture_with(policy(2, 1000, 1000)).await;
    failed_login(&fixture, "a@example.com", ip("2001:db8:1:2:aaaa::1")).await;
    failed_login(&fixture, "b@example.com", ip("2001:db8:1:2:bbbb::2")).await;

    assert!(matches!(
        failed_login(&fixture, "c@example.com", ip("2001:db8:1:2:cccc::3")).await,
        AuthError::TooManyAttempts { .. }
    ));
}

#[tokio::test]
async fn registration_is_limited_per_ip_and_every_attempt_counts() {
    let fixture = fixture_with(policy(1000, 1000, 2)).await;
    let client = ip("203.0.113.20");

    // 一次失败（邮箱不合法）和一次成功，都算在额度里。
    assert_eq!(
        fixture
            .service
            .register("bad", "correct-horse", client)
            .await,
        Err(AuthError::InvalidEmail)
    );
    assert!(
        fixture
            .service
            .register("new@example.com", "correct-horse", client)
            .await
            .is_ok()
    );

    let hash_calls_before = fixture.hasher.hash_calls.load(Ordering::SeqCst);
    let blocked = fixture
        .service
        .register("another@example.com", "correct-horse", client)
        .await;
    assert!(matches!(blocked, Err(AuthError::TooManyAttempts { .. })));
    assert_eq!(
        fixture.hasher.hash_calls.load(Ordering::SeqCst),
        hash_calls_before,
        "被限流时不能进入哈希"
    );

    assert!(
        fixture
            .service
            .register("another@example.com", "correct-horse", ip("203.0.113.21"))
            .await
            .is_ok(),
        "别的 IP 不受影响"
    );
}

#[tokio::test]
async fn retry_after_is_the_longest_remaining_wait_among_exceeded_limits() {
    // 登录窗口 15 分钟；这里只触发登录规则，等待时间就应该等于登录窗口。
    let fixture = fixture_with(policy(1, 1000, 1000)).await;
    failed_login(&fixture, "x@example.com", ip("203.0.113.30")).await;

    assert_eq!(
        failed_login(&fixture, "x@example.com", ip("203.0.113.30")).await,
        AuthError::TooManyAttempts {
            retry_after: WINDOW
        }
    );
}
