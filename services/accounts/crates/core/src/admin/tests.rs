//! 管理员域的测试：用内存替身验证规则，不连任何外部服务。

use std::sync::Arc;

use super::testing::InMemoryAdminRepository;
use super::*;
use crate::auth::testing::{
    InMemoryAttemptCounter, InMemorySessionStore, InMemoryUserRepository, PlainPasswordHasher,
};

const SESSION_TTL: Duration = Duration::from_secs(3600);
const WINDOW: Duration = Duration::from_secs(15 * 60);

struct Fixture {
    service: AdminService,
    admin_sessions: Arc<InMemorySessionStore>,
    users: Arc<InMemoryUserRepository>,
    attempt_counter: Arc<InMemoryAttemptCounter>,
}

fn throttle(per_ip: u32, per_account: u32) -> ThrottlePolicy {
    ThrottlePolicy {
        login_max_failures_per_ip: per_ip,
        login_max_failures_per_account: per_account,
        login_window: WINDOW,
        ..ThrottlePolicy::default()
    }
}

async fn fixture_with(throttle: ThrottlePolicy) -> Fixture {
    let admin_sessions = Arc::new(InMemorySessionStore::default());
    let users = Arc::new(InMemoryUserRepository::default());
    let attempt_counter = Arc::new(InMemoryAttemptCounter::default());
    let service = AdminService::new(
        AdminPorts {
            admin_repository: Arc::new(InMemoryAdminRepository::default()),
            user_repository: users.clone(),
            session_store: admin_sessions.clone(),
            password_hasher: Arc::new(PlainPasswordHasher),
            attempt_counter: attempt_counter.clone(),
        },
        AdminPolicy {
            session_ttl: SESSION_TTL,
            throttle,
        },
    )
    .await
    .unwrap();
    Fixture {
        service,
        admin_sessions,
        users,
        attempt_counter,
    }
}

async fn fixture() -> Fixture {
    fixture_with(throttle(1000, 1000)).await
}

async fn bootstrapped() -> Fixture {
    let fixture = fixture().await;
    fixture
        .service
        .bootstrap_admin("root", "correct-horse")
        .await
        .unwrap();
    fixture
}

fn ip(text: &str) -> Option<IpAddr> {
    Some(text.parse().unwrap())
}

async fn add_users(fixture: &Fixture, emails: &[&str]) {
    for email in emails {
        fixture.users.create(email, "hash").await.unwrap();
    }
}

// ---------- 用户名 ----------

#[test]
fn usernames_are_trimmed_lowercased_and_restricted() {
    assert_eq!(
        normalize_username("  Root.Admin ").as_deref(),
        Ok("root.admin")
    );
    assert_eq!(
        normalize_username("ops+1@example.com").as_deref(),
        Ok("ops+1@example.com")
    );
    for bad in [
        "",
        "   ",
        "has space",
        "tab\there",
        "引号'x",
        "a/b",
        "x;drop",
    ] {
        assert_eq!(
            normalize_username(bad),
            Err(AuthError::InvalidUsername),
            "{bad:?}"
        );
    }
    assert_eq!(
        normalize_username(&"a".repeat(MAX_USERNAME_BYTES + 1)),
        Err(AuthError::InvalidUsername)
    );
    assert!(normalize_username(&"a".repeat(MAX_USERNAME_BYTES)).is_ok());
}

// ---------- 引导 ----------

#[tokio::test]
async fn bootstrap_creates_the_first_admin_and_never_overwrites_an_existing_one() {
    let fixture = fixture().await;

    let first = fixture
        .service
        .bootstrap_admin(" Root ", "correct-horse")
        .await
        .unwrap();
    let BootstrapOutcome::Created(admin) = first else {
        panic!("第一次应当创建：{first:?}");
    };
    assert_eq!(
        (admin.username.as_str(), admin.display_name.as_str()),
        ("root", "root")
    );

    // 第二次带着不同的密码：什么都不能改，尤其不能覆盖密码。
    let second = fixture
        .service
        .bootstrap_admin("root", "a-different-password")
        .await;
    assert_eq!(second, Ok(BootstrapOutcome::AlreadyExists));
    assert!(
        fixture
            .service
            .login("root", "correct-horse", None)
            .await
            .is_ok(),
        "原密码仍然有效"
    );
    assert_eq!(
        fixture
            .service
            .login("root", "a-different-password", None)
            .await
            .map(|_| ()),
        Err(AuthError::InvalidCredentials),
        "引导不能被用来改密码"
    );
}

#[tokio::test]
async fn bootstrap_rejects_bad_usernames_and_weak_passwords() {
    let fixture = fixture().await;
    assert_eq!(
        fixture
            .service
            .bootstrap_admin("has space", "correct-horse")
            .await,
        Err(AuthError::InvalidUsername)
    );
    assert_eq!(
        fixture.service.bootstrap_admin("root", "short").await,
        Err(AuthError::PasswordTooShort)
    );
}

// ---------- 登录与会话 ----------

#[tokio::test]
async fn login_then_authenticate_then_logout() {
    let fixture = bootstrapped().await;

    let (admin, token) = fixture
        .service
        .login("  ROOT ", "correct-horse", None)
        .await
        .unwrap();
    assert_eq!(admin.username, "root");
    assert_eq!(fixture.service.authenticate(&token).await, Ok(admin));

    fixture.service.logout(&token).await.unwrap();
    assert_eq!(
        fixture.service.authenticate(&token).await,
        Err(AuthError::Unauthenticated)
    );
    assert_eq!(
        fixture.service.logout(&token).await,
        Ok(()),
        "重复退出是幂等的"
    );
}

#[tokio::test]
async fn unknown_username_and_wrong_password_are_indistinguishable_and_create_no_session() {
    let fixture = bootstrapped().await;

    let wrong_password = fixture.service.login("root", "wrong-password", None).await;
    let unknown_user = fixture.service.login("nobody", "correct-horse", None).await;
    let malformed = fixture
        .service
        .login("has space", "correct-horse", None)
        .await;
    let oversized = fixture.service.login("root", &"x".repeat(1000), None).await;

    for outcome in [wrong_password, unknown_user, malformed, oversized] {
        assert_eq!(outcome.map(|_| ()), Err(AuthError::InvalidCredentials));
    }
    assert_eq!(fixture.admin_sessions.active_session_count(), 0);
}

#[tokio::test]
async fn a_token_that_was_never_issued_does_not_authenticate() {
    let fixture = bootstrapped().await;
    assert_eq!(
        fixture
            .service
            .authenticate(&SessionToken::new("forged"))
            .await,
        Err(AuthError::Unauthenticated)
    );
}

// ---------- 限流 ----------

#[tokio::test]
async fn an_admin_account_is_throttled_and_even_the_right_password_is_refused() {
    let fixture = fixture_with(throttle(1000, 3)).await;
    fixture
        .service
        .bootstrap_admin("root", "correct-horse")
        .await
        .unwrap();
    for attacker in ["198.51.100.1", "198.51.100.2", "198.51.100.3"] {
        let outcome = fixture
            .service
            .login("root", "wrong-password", ip(attacker))
            .await;
        assert_eq!(outcome.map(|_| ()), Err(AuthError::InvalidCredentials));
    }

    let blocked = fixture
        .service
        .login("root", "correct-horse", ip("198.51.100.99"))
        .await;

    assert_eq!(
        blocked.map(|_| ()),
        Err(AuthError::TooManyAttempts {
            retry_after: WINDOW
        })
    );
    assert_eq!(fixture.admin_sessions.active_session_count(), 0);
}

#[tokio::test]
async fn admin_and_user_login_counters_never_affect_each_other() {
    let fixture = fixture_with(throttle(3, 3)).await;
    fixture
        .service
        .bootstrap_admin("root", "correct-horse")
        .await
        .unwrap();
    let attacker = ip("203.0.113.5");

    // 把普通用户域里同名账号、同一个 IP 的失败计数刷到上限（键前缀是 `login`）。
    for _ in 0..3 {
        fixture
            .attempt_counter
            .increment("login:account:root", WINDOW)
            .await
            .unwrap();
        fixture
            .attempt_counter
            .increment("login:ip:203.0.113.5", WINDOW)
            .await
            .unwrap();
    }

    assert!(
        fixture
            .service
            .login("root", "correct-horse", attacker)
            .await
            .is_ok(),
        "刷普通登录的失败次数，不能锁住管理员"
    );

    // 反过来：管理员域被刷满后，普通域的键不受影响。
    for _ in 0..3 {
        let _ = fixture
            .service
            .login("root", "wrong-password", attacker)
            .await;
    }
    assert_eq!(
        fixture
            .attempt_counter
            .status("login:account:root")
            .await
            .unwrap()
            .map(|s| s.count),
        Some(3)
    );
    assert_eq!(
        fixture
            .attempt_counter
            .status("admin-login:account:root")
            .await
            .unwrap()
            .map(|s| s.count),
        Some(3)
    );
}

#[tokio::test]
async fn success_clears_the_admin_account_counter_but_not_the_ip_counter() {
    let fixture = fixture_with(throttle(10, 10)).await;
    fixture
        .service
        .bootstrap_admin("root", "correct-horse")
        .await
        .unwrap();
    let client = ip("203.0.113.7");
    let _ = fixture
        .service
        .login("root", "wrong-password", client)
        .await;
    let _ = fixture
        .service
        .login("root", "wrong-password", client)
        .await;

    fixture
        .service
        .login("root", "correct-horse", client)
        .await
        .unwrap();

    assert_eq!(
        fixture
            .attempt_counter
            .status("admin-login:account:root")
            .await,
        Ok(None)
    );
    assert_eq!(
        fixture
            .attempt_counter
            .status("admin-login:ip:203.0.113.7")
            .await
            .unwrap()
            .map(|s| s.count),
        Some(2),
        "IP 计数不能被登录成功清零"
    );
}

// ---------- 用户管理 ----------

#[tokio::test]
async fn listing_users_paginates_and_reports_the_total() {
    let fixture = bootstrapped().await;
    add_users(
        &fixture,
        &["a@x.com", "b@x.com", "c@x.com", "d@x.com", "e@x.com"],
    )
    .await;

    let first = fixture
        .service
        .list_users(UserListRequest {
            page: Some(1),
            page_size: Some(2),
            ..Default::default()
        })
        .await
        .unwrap();
    let last = fixture
        .service
        .list_users(UserListRequest {
            page: Some(3),
            page_size: Some(2),
            ..Default::default()
        })
        .await
        .unwrap();
    let beyond = fixture
        .service
        .list_users(UserListRequest {
            page: Some(9),
            page_size: Some(2),
            ..Default::default()
        })
        .await
        .unwrap();

    let emails = |page: &UserPage| {
        page.items
            .iter()
            .map(|user| user.email.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        (emails(&first), first.total),
        (vec!["a@x.com".into(), "b@x.com".into()], 5)
    );
    assert_eq!((emails(&last), last.total), (vec!["e@x.com".into()], 5));
    assert_eq!(
        (emails(&beyond), beyond.total),
        (Vec::<String>::new(), 5),
        "越界的页是空页，但总数仍然正确"
    );
}

#[tokio::test]
async fn paging_inputs_are_clamped_instead_of_trusted() {
    let fixture = bootstrapped().await;
    let emails: Vec<String> = (0..150).map(|index| format!("user{index}@x.com")).collect();
    add_users(
        &fixture,
        &emails.iter().map(String::as_str).collect::<Vec<_>>(),
    )
    .await;

    let huge = fixture
        .service
        .list_users(UserListRequest {
            page_size: Some(100_000),
            ..Default::default()
        })
        .await
        .unwrap();
    let zero_size = fixture
        .service
        .list_users(UserListRequest {
            page_size: Some(0),
            ..Default::default()
        })
        .await
        .unwrap();
    let zero_page = fixture
        .service
        .list_users(UserListRequest {
            page: Some(0),
            ..Default::default()
        })
        .await
        .unwrap();
    let defaults = fixture
        .service
        .list_users(UserListRequest::default())
        .await
        .unwrap();

    assert_eq!(
        huge.items.len(),
        MAX_PAGE_SIZE as usize,
        "不能让调用方一次拉走整张表"
    );
    assert_eq!(zero_size.items.len(), 1);
    assert_eq!(
        zero_page.items.first().map(|u| u.email.as_str()),
        Some("user0@x.com"),
        "第 0 页按第 1 页算"
    );
    assert_eq!(defaults.items.len(), DEFAULT_PAGE_SIZE as usize);
    assert_eq!(defaults.total, 150);
}

#[tokio::test]
async fn listing_filters_by_status_and_by_a_case_insensitive_trimmed_keyword() {
    let fixture = bootstrapped().await;
    add_users(
        &fixture,
        &["alice@example.com", "bob@example.com", "carol@other.org"],
    )
    .await;
    fixture
        .service
        .set_user_status(2, UserStatus::Disabled)
        .await
        .unwrap();

    let disabled = fixture
        .service
        .list_users(UserListRequest {
            status: Some(UserStatus::Disabled),
            ..Default::default()
        })
        .await
        .unwrap();
    let by_keyword = fixture
        .service
        .list_users(UserListRequest {
            keyword: Some("  EXAMPLE ".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let blank_keyword = fixture
        .service
        .list_users(UserListRequest {
            keyword: Some("   ".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let both = fixture
        .service
        .list_users(UserListRequest {
            status: Some(UserStatus::Active),
            keyword: Some("example".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(
        disabled
            .items
            .iter()
            .map(|u| u.email.as_str())
            .collect::<Vec<_>>(),
        ["bob@example.com"]
    );
    assert_eq!(by_keyword.total, 2);
    assert_eq!(blank_keyword.total, 3, "纯空白的关键字等于没有关键字");
    assert_eq!(
        both.items
            .iter()
            .map(|u| u.email.as_str())
            .collect::<Vec<_>>(),
        ["alice@example.com"]
    );
}

#[tokio::test]
async fn enabling_and_disabling_is_idempotent_and_unknown_users_are_none() {
    let fixture = bootstrapped().await;
    add_users(&fixture, &["a@x.com"]).await;

    let disabled = fixture
        .service
        .set_user_status(1, UserStatus::Disabled)
        .await
        .unwrap()
        .unwrap();
    let again = fixture
        .service
        .set_user_status(1, UserStatus::Disabled)
        .await
        .unwrap()
        .unwrap();
    let enabled = fixture
        .service
        .set_user_status(1, UserStatus::Active)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        (disabled.status, again.status, enabled.status),
        (
            UserStatus::Disabled,
            UserStatus::Disabled,
            UserStatus::Active
        )
    );
    assert_eq!(
        fixture.service.get_user(1).await.unwrap().unwrap().status,
        UserStatus::Active
    );
    assert_eq!(
        fixture
            .service
            .set_user_status(999, UserStatus::Disabled)
            .await,
        Ok(None)
    );
    assert_eq!(fixture.service.get_user(999).await, Ok(None));
}

#[test]
fn credentials_never_appear_in_debug_output() {
    let credentials = AdminCredentials {
        admin: Admin {
            id: AdminId(1),
            username: "root".into(),
            display_name: "root".into(),
        },
        password_hash: "$argon2id$secret-hash".into(),
    };
    assert!(!format!("{credentials:?}").contains("secret-hash"));
}
