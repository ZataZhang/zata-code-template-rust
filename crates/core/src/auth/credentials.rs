//! 普通用户域与管理员域共用的登录辅助：限流器、诱饵哈希校验。
//!
//! 两个域的登录流程几乎一样（规范化标识 → 检查限流 → 取凭据 → 校验密码 → 计数），区别只在标识怎么规范化、
//! 凭据从哪张表取、限流键的前缀。把共用的两块抽出来，避免复制粘贴出两份会各自漂移的安全逻辑。

use std::sync::Arc;
use std::time::Duration;

use super::throttle::Limit;
use super::{AttemptCounter, AuthError, PasswordHasher, PortFailure};

/// 限流器：在一组规则上检查、计数、清零。
pub(crate) struct Throttler {
    counter: Arc<dyn AttemptCounter>,
}

impl Throttler {
    pub(crate) fn new(counter: Arc<dyn AttemptCounter>) -> Self {
        Self { counter }
    }

    /// 任何一条规则已达上限就拒绝，并告知最长还需等待多久。
    pub(crate) async fn ensure_within_limits(&self, limits: &[Limit]) -> Result<(), AuthError> {
        let mut longest_wait: Option<Duration> = None;
        for limit in limits {
            let status = self.counter.status(&limit.key).await?;
            if let Some(status) = status.filter(|status| status.count >= limit.max) {
                longest_wait =
                    Some(longest_wait.map_or(status.resets_in, |wait| wait.max(status.resets_in)));
            }
        }
        match longest_wait {
            Some(retry_after) => Err(AuthError::TooManyAttempts { retry_after }),
            None => Ok(()),
        }
    }

    /// 给每条规则的计数加一。
    pub(crate) async fn record(&self, limits: &[Limit]) -> Result<(), AuthError> {
        for limit in limits {
            self.counter.increment(&limit.key, limit.window).await?;
        }
        Ok(())
    }

    /// 清零一个键。
    pub(crate) async fn reset(&self, key: &str) -> Result<(), AuthError> {
        Ok(self.counter.reset(key).await?)
    }
}

/// 校验密码，且无论账号是否存在都做**一次**哈希校验，让响应时间不泄露账号是否存在。
///
/// `stored_hash` 为 `None`（账号不存在）时拿密码去校验诱饵哈希，结果一律是「不匹配」。
/// 返回「账号存在且密码匹配」。
pub(crate) async fn verify_with_decoy(
    hasher: &dyn PasswordHasher,
    decoy_hash: &str,
    stored_hash: Option<&str>,
    password: &str,
) -> Result<bool, PortFailure> {
    match stored_hash {
        Some(hash) => hasher.verify(password, hash).await,
        None => {
            hasher.verify(password, decoy_hash).await?;
            Ok(false)
        }
    }
}
