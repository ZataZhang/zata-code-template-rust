//! Redis：会话存储与就绪探针。

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use sha2::{Digest, Sha256};
use tokio::sync::OnceCell;
use tpl_core::auth::{PortFailure, SessionStore, SessionToken};
use tpl_core::health::ReadinessProbe;

/// 令牌的随机字节数：32 字节 = 256 位，无法被猜出。
const TOKEN_RANDOM_BYTES: usize = 32;

/// 惰性建立的 Redis 连接（自动重连），由会话存储和就绪探针共用。
///
/// 和 PostgreSQL 连接池一样是**惰性**的：Redis 暂时不可用时服务照常启动，
/// 首次真正使用时才连接；连接失败不会被缓存，下次调用会重试。
pub struct RedisConnection {
    client: redis::Client,
    connection_manager: OnceCell<ConnectionManager>,
    operation_timeout: Duration,
}

impl RedisConnection {
    /// 创建（但还不连接）。`operation_timeout` 约束每次操作，包括首次建立连接。
    ///
    /// # Errors
    ///
    /// 连接串无法解析时返回错误。
    pub fn new(redis_url: &str, operation_timeout: Duration) -> Result<Self, redis::RedisError> {
        Ok(Self {
            client: redis::Client::open(redis_url)?,
            connection_manager: OnceCell::new(),
            operation_timeout,
        })
    }

    /// 取得（必要时先建立）连接并执行 `operation`，整体带超时。失败原因只用于服务端日志。
    async fn run<T, Operation, Pending>(&self, operation: Operation) -> Result<T, PortFailure>
    where
        Operation: FnOnce(ConnectionManager) -> Pending,
        Pending: std::future::Future<Output = redis::RedisResult<T>>,
    {
        let attempt = async {
            let connection = self
                .connection_manager
                .get_or_try_init(|| ConnectionManager::new(self.client.clone()))
                .await?
                .clone();
            operation(connection).await
        };
        match tokio::time::timeout(self.operation_timeout, attempt).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(redis_error)) => Err(PortFailure(format!("Redis 操作失败：{redis_error}"))),
            Err(_elapsed) => Err(PortFailure(format!(
                "Redis 操作超时（{:?}）",
                self.operation_timeout
            ))),
        }
    }
}

impl RedisConnection {
    /// 执行一条命令，带超时。
    pub(crate) async fn query<T: redis::FromRedisValue>(
        &self,
        command: redis::Cmd,
    ) -> Result<T, PortFailure> {
        self.run(|mut connection| async move { command.query_async::<T>(&mut connection).await })
            .await
    }

    /// 执行一个管道（可用 `atomic()` 变成 MULTI/EXEC 事务），带超时。
    pub(crate) async fn query_pipeline<T: redis::FromRedisValue>(
        &self,
        pipeline: redis::Pipeline,
    ) -> Result<T, PortFailure> {
        self.run(|mut connection| async move { pipeline.query_async::<T>(&mut connection).await })
            .await
    }
}

/// 把字节转成小写十六进制。
fn to_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut hex, byte| {
            // 写入 String 不会失败。
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// 字符串的 SHA-256，小写十六进制。Redis 键里凡是可能含敏感信息的部分（令牌、邮箱）都先过它。
pub(crate) fn sha256_hex(text: &str) -> String {
    to_hex(Sha256::digest(text.as_bytes()).as_slice())
}

/// 普通用户会话的键命名空间。
pub const USER_SESSION_NAMESPACE: &str = "session";
/// 管理员会话的键命名空间：与用户域不同，所以一个域签发的令牌在另一个域里永远查不到。
pub const ADMIN_SESSION_NAMESPACE: &str = "admin_session";

/// Redis 里的键：命名空间 + 对令牌取 SHA-256，**不存令牌本身**。
/// 这样即使 Redis 数据泄露，攻击者拿到的哈希也无法当作 Cookie 使用。
fn session_key(namespace: &str, session_token: &SessionToken) -> String {
    format!("{namespace}:{}", sha256_hex(session_token.as_str()))
}

/// 基于 Redis 的会话存储：键为命名空间加令牌的哈希，值为主体 ID，用 `EX` 自动过期。
pub struct RedisSessionStore {
    redis_connection: Arc<RedisConnection>,
    namespace: &'static str,
}

impl RedisSessionStore {
    /// 基于共享的 Redis 连接创建会话存储；`namespace` 决定键前缀，用来隔离不同的登录域。
    pub fn new(redis_connection: Arc<RedisConnection>, namespace: &'static str) -> Self {
        Self {
            redis_connection,
            namespace,
        }
    }
}

#[async_trait]
impl SessionStore for RedisSessionStore {
    async fn create(&self, principal_id: i64, ttl: Duration) -> Result<SessionToken, PortFailure> {
        let mut random_bytes = [0_u8; TOKEN_RANDOM_BYTES];
        getrandom::fill(&mut random_bytes)
            .map_err(|error| PortFailure(format!("无法获取系统随机数：{error}")))?;
        let session_token = SessionToken::new(to_hex(&random_bytes));

        let mut set_command = redis::cmd("SET");
        set_command
            .arg(session_key(self.namespace, &session_token))
            .arg(principal_id)
            .arg("EX")
            .arg(ttl.as_secs().max(1));
        self.redis_connection.query::<()>(set_command).await?;
        Ok(session_token)
    }

    async fn resolve(&self, token: &SessionToken) -> Result<Option<i64>, PortFailure> {
        let mut get_command = redis::cmd("GET");
        get_command.arg(session_key(self.namespace, token));
        self.redis_connection.query(get_command).await
    }

    async fn revoke(&self, token: &SessionToken) -> Result<(), PortFailure> {
        let mut delete_command = redis::cmd("DEL");
        delete_command.arg(session_key(self.namespace, token));
        self.redis_connection.query::<()>(delete_command).await
    }
}

/// Redis 就绪探针：发送 `PING`。
pub struct RedisReadinessProbe {
    redis_connection: Arc<RedisConnection>,
}

impl RedisReadinessProbe {
    /// 基于共享的 Redis 连接创建探针。
    pub fn new(redis_connection: Arc<RedisConnection>) -> Self {
        Self { redis_connection }
    }
}

#[async_trait]
impl ReadinessProbe for RedisReadinessProbe {
    fn dependency_name(&self) -> &'static str {
        "redis"
    }

    async fn check(&self) -> Result<(), String> {
        let reply: String = self
            .redis_connection
            .query(redis::cmd("PING"))
            .await
            .map_err(|failure| failure.0)?;
        if reply == "PONG" {
            Ok(())
        } else {
            Err(format!("PING 返回了意外的内容：{reply}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_key_hides_the_token_and_is_deterministic() {
        let session_token = SessionToken::new("my-secret-token");
        let key = session_key(USER_SESSION_NAMESPACE, &session_token);

        assert!(key.starts_with("session:"));
        assert!(!key.contains("my-secret-token"));
        assert_eq!(
            key.len(),
            "session:".len() + 64,
            "SHA-256 的十六进制是 64 个字符"
        );
        assert_eq!(
            key,
            session_key(
                USER_SESSION_NAMESPACE,
                &SessionToken::new("my-secret-token")
            )
        );
        assert_ne!(
            key,
            session_key(USER_SESSION_NAMESPACE, &SessionToken::new("another-token"))
        );
        assert_ne!(
            key,
            session_key(ADMIN_SESSION_NAMESPACE, &session_token),
            "同一个令牌在不同命名空间里是不同的键"
        );
    }

    #[test]
    fn hex_encoding_is_lowercase_and_zero_padded() {
        assert_eq!(to_hex(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
    }

    #[tokio::test]
    async fn probe_reports_failure_when_redis_is_unreachable() {
        // 1 号端口基本不会有人监听：连接立刻被拒绝，不需要任何外部服务。
        let redis_connection = Arc::new(
            RedisConnection::new("redis://127.0.0.1:1/0", Duration::from_secs(2)).unwrap(),
        );
        let probe = RedisReadinessProbe::new(redis_connection);

        assert!(probe.check().await.is_err());
        assert_eq!(probe.dependency_name(), "redis");
    }

    #[tokio::test]
    async fn malformed_redis_url_is_rejected_up_front() {
        assert!(RedisConnection::new("not a url", Duration::from_secs(1)).is_err());
    }

    fn real_redis_url() -> String {
        std::env::var("TEST_REDIS_URL").expect("请设置 TEST_REDIS_URL")
    }

    /// 真实 Redis 测试：默认跳过，用 `just test real` 显式运行。
    #[tokio::test]
    #[ignore = "需要真实 Redis，设置 TEST_REDIS_URL 后用 `just test real` 运行"]
    async fn session_lifecycle_against_a_real_redis() {
        let redis_connection =
            Arc::new(RedisConnection::new(&real_redis_url(), Duration::from_secs(5)).unwrap());
        let session_store =
            RedisSessionStore::new(redis_connection.clone(), USER_SESSION_NAMESPACE);

        let session_token = session_store
            .create(42, Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(session_token.as_str().len(), TOKEN_RANDOM_BYTES * 2);
        assert_eq!(session_store.resolve(&session_token).await, Ok(Some(42)));

        // 令牌本身不能是 Redis 里的键：直接拿它去查应该查不到。
        let mut raw_lookup = redis::cmd("GET");
        raw_lookup.arg(format!("session:{}", session_token.as_str()));
        let raw_value: Option<String> = redis_connection.query(raw_lookup).await.unwrap();
        assert_eq!(raw_value, None);

        session_store.revoke(&session_token).await.unwrap();
        assert_eq!(session_store.resolve(&session_token).await, Ok(None));
        assert_eq!(
            session_store.revoke(&session_token).await,
            Ok(()),
            "重复撤销是幂等的"
        );
    }

    /// 两个命名空间共用同一个 Redis：一个域签发的令牌在另一个域里查不到，撤销也互不影响。
    #[tokio::test]
    #[ignore = "需要真实 Redis，设置 TEST_REDIS_URL 后用 `just test real` 运行"]
    async fn namespaces_isolate_user_and_admin_sessions() {
        let redis_connection =
            Arc::new(RedisConnection::new(&real_redis_url(), Duration::from_secs(5)).unwrap());
        let user_sessions =
            RedisSessionStore::new(redis_connection.clone(), USER_SESSION_NAMESPACE);
        let admin_sessions = RedisSessionStore::new(redis_connection, ADMIN_SESSION_NAMESPACE);

        let user_token = user_sessions
            .create(1, Duration::from_secs(60))
            .await
            .unwrap();
        let admin_token = admin_sessions
            .create(1, Duration::from_secs(60))
            .await
            .unwrap();

        // 主体 ID 相同（都是 1），令牌互相拿去对方那里也必须查不到。
        assert_eq!(admin_sessions.resolve(&user_token).await, Ok(None));
        assert_eq!(user_sessions.resolve(&admin_token).await, Ok(None));
        // 对方域里撤销别域的令牌是空操作，不会误删。
        admin_sessions.revoke(&user_token).await.unwrap();
        assert_eq!(user_sessions.resolve(&user_token).await, Ok(Some(1)));

        user_sessions.revoke(&user_token).await.unwrap();
        admin_sessions.revoke(&admin_token).await.unwrap();
    }

    #[tokio::test]
    #[ignore = "需要真实 Redis，设置 TEST_REDIS_URL 后用 `just test real` 运行"]
    async fn sessions_expire_after_their_ttl() {
        let redis_connection =
            Arc::new(RedisConnection::new(&real_redis_url(), Duration::from_secs(5)).unwrap());
        let session_store = RedisSessionStore::new(redis_connection, USER_SESSION_NAMESPACE);

        let session_token = session_store
            .create(7, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(session_store.resolve(&session_token).await, Ok(Some(7)));

        tokio::time::sleep(Duration::from_millis(1600)).await;
        assert_eq!(session_store.resolve(&session_token).await, Ok(None));
    }

    #[tokio::test]
    #[ignore = "需要真实 Redis，设置 TEST_REDIS_URL 后用 `just test real` 运行"]
    async fn probe_passes_against_a_real_redis() {
        let redis_connection =
            Arc::new(RedisConnection::new(&real_redis_url(), Duration::from_secs(5)).unwrap());
        assert_eq!(
            RedisReadinessProbe::new(redis_connection).check().await,
            Ok(())
        );
    }
}
