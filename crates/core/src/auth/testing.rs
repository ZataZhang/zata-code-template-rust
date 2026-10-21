//! 认证端口的内存替身，**只用于测试**。
//!
//! 其中的 [`PlainPasswordHasher`] 不做任何加密，绝不能用于生产。

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;

use super::{
    AttemptCounter, AttemptStatus, PasswordHasher, PortFailure, SessionStore, SessionToken,
    StoredCredentials, User, UserId, UserListQuery, UserPage, UserRepository, UserRepositoryError,
    UserStatus,
};

/// 内存版用户存储。
#[derive(Default)]
pub struct InMemoryUserRepository {
    rows: Mutex<Vec<StoredCredentials>>,
}

#[async_trait]
impl UserRepository for InMemoryUserRepository {
    async fn create(&self, email: &str, password_hash: &str) -> Result<User, UserRepositoryError> {
        let mut rows = self.rows.lock().expect("测试替身的锁不应被毒化");
        if rows.iter().any(|row| row.user.email == email) {
            return Err(UserRepositoryError::EmailTaken);
        }
        let user = User {
            id: UserId(i64::try_from(rows.len()).expect("测试里用户数不会溢出") + 1),
            email: email.to_string(),
            status: UserStatus::Active,
            created_at: "2026-01-01T00:00:00Z".to_string(),
        };
        rows.push(StoredCredentials {
            user: user.clone(),
            password_hash: password_hash.to_string(),
        });
        Ok(user)
    }

    async fn find_credentials_by_email(
        &self,
        email: &str,
    ) -> Result<Option<StoredCredentials>, UserRepositoryError> {
        let rows = self.rows.lock().expect("测试替身的锁不应被毒化");
        Ok(rows.iter().find(|row| row.user.email == email).cloned())
    }

    async fn find_by_id(&self, user_id: UserId) -> Result<Option<User>, UserRepositoryError> {
        let rows = self.rows.lock().expect("测试替身的锁不应被毒化");
        Ok(rows
            .iter()
            .find(|row| row.user.id == user_id)
            .map(|row| row.user.clone()))
    }

    async fn list(&self, query: &UserListQuery) -> Result<UserPage, UserRepositoryError> {
        let rows = self.rows.lock().expect("测试替身的锁不应被毒化");
        let keyword = query.keyword.as_deref().map(str::to_lowercase);
        let matching: Vec<User> = rows
            .iter()
            .map(|row| &row.user)
            .filter(|user| query.status.is_none_or(|status| user.status == status))
            .filter(|user| {
                keyword
                    .as_deref()
                    .is_none_or(|keyword| user.email.to_lowercase().contains(keyword))
            })
            .cloned()
            .collect();
        let skip = (query.page.saturating_sub(1) as usize) * query.page_size as usize;
        Ok(UserPage {
            total: matching.len() as u64,
            items: matching
                .into_iter()
                .skip(skip)
                .take(query.page_size as usize)
                .collect(),
        })
    }

    async fn set_status(
        &self,
        user_id: UserId,
        status: UserStatus,
    ) -> Result<Option<User>, UserRepositoryError> {
        let mut rows = self.rows.lock().expect("测试替身的锁不应被毒化");
        Ok(rows
            .iter_mut()
            .find(|row| row.user.id == user_id)
            .map(|row| {
                row.user.status = status;
                row.user.clone()
            }))
    }
}

/// 内存版会话存储。令牌是递增编号，不可猜测性不在这里验证；`ttl` 被忽略。
#[derive(Default)]
pub struct InMemorySessionStore {
    sessions: Mutex<HashMap<String, i64>>,
    next_token_number: AtomicU64,
}

impl InMemorySessionStore {
    /// 当前有效会话数，供测试断言。
    pub fn active_session_count(&self) -> usize {
        self.sessions.lock().expect("测试替身的锁不应被毒化").len()
    }
}

#[async_trait]
impl SessionStore for InMemorySessionStore {
    async fn create(&self, principal_id: i64, _ttl: Duration) -> Result<SessionToken, PortFailure> {
        let token_number = self.next_token_number.fetch_add(1, Ordering::Relaxed);
        let token_text = format!("test-session-{token_number}");
        self.sessions
            .lock()
            .expect("测试替身的锁不应被毒化")
            .insert(token_text.clone(), principal_id);
        Ok(SessionToken::new(token_text))
    }

    async fn resolve(&self, token: &SessionToken) -> Result<Option<i64>, PortFailure> {
        Ok(self
            .sessions
            .lock()
            .expect("测试替身的锁不应被毒化")
            .get(token.as_str())
            .copied())
    }

    async fn revoke(&self, token: &SessionToken) -> Result<(), PortFailure> {
        self.sessions
            .lock()
            .expect("测试替身的锁不应被毒化")
            .remove(token.as_str());
        Ok(())
    }
}

/// 明文「哈希器」：只给密码加个前缀。**不安全，仅用于测试。**
pub struct PlainPasswordHasher;

#[async_trait]
impl PasswordHasher for PlainPasswordHasher {
    async fn hash(&self, password: &str) -> Result<String, PortFailure> {
        Ok(format!("plain${password}"))
    }

    async fn verify(&self, password: &str, stored_hash: &str) -> Result<bool, PortFailure> {
        Ok(stored_hash == format!("plain${password}"))
    }
}

/// 内存版尝试次数计数器。不模拟时间流逝：窗口永不结束，`resets_in` 返回创建时给的窗口长度。
#[derive(Default)]
pub struct InMemoryAttemptCounter {
    counters: Mutex<HashMap<String, (u32, Duration)>>,
}

#[async_trait]
impl AttemptCounter for InMemoryAttemptCounter {
    async fn status(&self, key: &str) -> Result<Option<AttemptStatus>, PortFailure> {
        let counters = self.counters.lock().expect("测试替身的锁不应被毒化");
        Ok(counters.get(key).map(|(count, window)| AttemptStatus {
            count: *count,
            resets_in: *window,
        }))
    }

    async fn increment(&self, key: &str, window: Duration) -> Result<u32, PortFailure> {
        let mut counters = self.counters.lock().expect("测试替身的锁不应被毒化");
        let entry = counters.entry(key.to_string()).or_insert((0, window));
        entry.0 += 1;
        Ok(entry.0)
    }

    async fn reset(&self, key: &str) -> Result<(), PortFailure> {
        self.counters
            .lock()
            .expect("测试替身的锁不应被毒化")
            .remove(key);
        Ok(())
    }
}
