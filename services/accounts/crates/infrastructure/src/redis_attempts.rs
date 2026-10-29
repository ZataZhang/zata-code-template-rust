//! Redis：尝试次数计数器（登录、注册限流用的固定窗口计数）。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tpl_core::auth::{AttemptCounter, AttemptStatus, PortFailure};

use crate::redis_session::{RedisConnection, sha256_hex};

/// Redis 里的键。限流键里含邮箱和 IP，所以**取哈希后再存**：Redis 里看不到明文的个人信息。
fn counter_key(logical_key: &str) -> String {
    format!("throttle:{}", sha256_hex(logical_key))
}

/// 基于 Redis 的固定窗口计数器。
pub struct RedisAttemptCounter {
    redis_connection: Arc<RedisConnection>,
}

impl RedisAttemptCounter {
    /// 基于共享的 Redis 连接创建计数器。
    pub fn new(redis_connection: Arc<RedisConnection>) -> Self {
        Self { redis_connection }
    }
}

#[async_trait]
impl AttemptCounter for RedisAttemptCounter {
    async fn status(&self, key: &str) -> Result<Option<AttemptStatus>, PortFailure> {
        let redis_key = counter_key(key);
        let mut pipeline = redis::pipe();
        pipeline
            .cmd("GET")
            .arg(&redis_key)
            .cmd("PTTL")
            .arg(&redis_key);
        let (count, remaining_millis): (Option<u32>, i64) =
            self.redis_connection.query_pipeline(pipeline).await?;

        Ok(count.map(|count| AttemptStatus {
            count,
            // PTTL 在键没有过期时间时返回 -1：不应发生，按「马上结束」处理而不是永久封禁。
            resets_in: Duration::from_millis(u64::try_from(remaining_millis).unwrap_or(0)),
        }))
    }

    async fn increment(&self, key: &str, window: Duration) -> Result<u32, PortFailure> {
        let redis_key = counter_key(key);
        // 在一个 MULTI/EXEC 事务里：键不存在才创建并设置过期（SET NX EX），然后 INCR。
        // 不能拆成「INCR 然后 EXPIRE」：进程恰好在两步之间崩溃，就会留下一个永不过期的计数键，
        // 等于永久封禁。已存在的键不会被 SET NX 改动，所以窗口是固定的、不会被越拖越长。
        let mut pipeline = redis::pipe();
        pipeline
            .atomic()
            .cmd("SET")
            .arg(&redis_key)
            .arg(0)
            .arg("EX")
            .arg(window.as_secs().max(1))
            .arg("NX")
            .ignore()
            .cmd("INCR")
            .arg(&redis_key);
        let (new_count,): (u32,) = self.redis_connection.query_pipeline(pipeline).await?;
        Ok(new_count)
    }

    async fn reset(&self, key: &str) -> Result<(), PortFailure> {
        let mut delete_command = redis::cmd("DEL");
        delete_command.arg(counter_key(key));
        self.redis_connection.query::<()>(delete_command).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn real_redis_connection() -> Arc<RedisConnection> {
        let redis_url = std::env::var("TEST_REDIS_URL").expect("请设置 TEST_REDIS_URL");
        Arc::new(RedisConnection::new(&redis_url, Duration::from_secs(5)).unwrap())
    }

    /// 每次运行用不同的键，互不干扰，也不依赖库里已有的数据。
    fn unique_key(label: &str) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("test:{label}:{nanos}")
    }

    #[test]
    fn redis_key_never_contains_the_logical_key() {
        let redis_key = counter_key("login:account:alice@example.com");
        assert!(redis_key.starts_with("throttle:"));
        assert!(!redis_key.contains("alice"));
        assert_eq!(redis_key.len(), "throttle:".len() + 64);
    }

    #[tokio::test]
    async fn unreachable_redis_is_an_error_not_a_silent_zero() {
        let unreachable = Arc::new(
            RedisConnection::new("redis://127.0.0.1:1/0", Duration::from_secs(2)).unwrap(),
        );
        let counter = RedisAttemptCounter::new(unreachable);
        // 限流器失败时不能悄悄当作「没有尝试过」，否则 Redis 一抖限流就全失效了。
        assert!(counter.status("k").await.is_err());
        assert!(
            counter
                .increment("k", Duration::from_secs(60))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    #[ignore = "需要真实 Redis，设置 TEST_REDIS_URL 后用 `just test real` 运行"]
    async fn counts_up_reports_the_window_and_resets() {
        let counter = RedisAttemptCounter::new(real_redis_connection());
        let key = unique_key("count");

        assert_eq!(counter.status(&key).await, Ok(None));
        assert_eq!(
            counter.increment(&key, Duration::from_secs(60)).await,
            Ok(1)
        );
        assert_eq!(
            counter.increment(&key, Duration::from_secs(60)).await,
            Ok(2)
        );
        assert_eq!(
            counter.increment(&key, Duration::from_secs(60)).await,
            Ok(3)
        );

        let status = counter.status(&key).await.unwrap().unwrap();
        assert_eq!(status.count, 3);
        assert!(
            status.resets_in > Duration::from_secs(50)
                && status.resets_in <= Duration::from_secs(60),
            "窗口剩余时间应接近 60 秒，实际 {:?}",
            status.resets_in
        );

        counter.reset(&key).await.unwrap();
        assert_eq!(counter.status(&key).await, Ok(None));
        assert_eq!(counter.reset(&key).await, Ok(()), "重复清零是幂等的");
    }

    #[tokio::test]
    #[ignore = "需要真实 Redis，设置 TEST_REDIS_URL 后用 `just test real` 运行"]
    async fn the_window_is_fixed_and_not_extended_by_later_attempts() {
        let counter = RedisAttemptCounter::new(real_redis_connection());
        let key = unique_key("fixed");

        counter
            .increment(&key, Duration::from_secs(2))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(1200)).await;
        // 第二次尝试带着更长的窗口参数，但不应该延长已经开启的窗口。
        counter
            .increment(&key, Duration::from_secs(600))
            .await
            .unwrap();

        let status = counter.status(&key).await.unwrap().unwrap();
        assert_eq!(status.count, 2);
        assert!(
            status.resets_in <= Duration::from_millis(900),
            "窗口被延长了：{:?}",
            status.resets_in
        );

        tokio::time::sleep(Duration::from_millis(1100)).await;
        assert_eq!(
            counter.status(&key).await,
            Ok(None),
            "窗口结束后计数自动归零"
        );
    }

    #[tokio::test]
    #[ignore = "需要真实 Redis，设置 TEST_REDIS_URL 后用 `just test real` 运行"]
    async fn concurrent_increments_are_not_lost() {
        let counter = Arc::new(RedisAttemptCounter::new(real_redis_connection()));
        let key = unique_key("concurrent");

        let mut tasks = Vec::new();
        for _ in 0..20 {
            let counter = counter.clone();
            let key = key.clone();
            tasks.push(tokio::spawn(async move {
                counter
                    .increment(&key, Duration::from_secs(60))
                    .await
                    .unwrap()
            }));
        }
        let mut returned_values = Vec::new();
        for task in tasks {
            returned_values.push(task.await.unwrap());
        }
        returned_values.sort_unstable();

        assert_eq!(
            returned_values,
            (1..=20).collect::<Vec<u32>>(),
            "每次递增都应拿到唯一的新值"
        );
        counter.reset(&key).await.unwrap();
    }
}
