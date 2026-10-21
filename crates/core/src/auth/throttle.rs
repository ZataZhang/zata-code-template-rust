//! 登录与注册限流：策略、计数端口，以及限流键的构造。
//!
//! 目标是让「无限次猜密码」和「批量注册」变得不划算，同时不让攻击者借限流本身
//! 探测「某个邮箱是否已注册」。设计要点：
//!
//! - **按客户端 IP** 限制失败次数，挡住拿同一批密码撞很多账号的撞库；
//! - **按账号** 限制失败次数，挡住分布在很多 IP 上的慢速猜测；
//! - **不存在的邮箱同样计数**，所以「锁定」行为不会泄露账号是否存在；
//! - 登录成功只清零**账号**计数，**不清零 IP 计数**，否则攻击者拿自己的账号登录一次就能重置 IP 计数；
//! - 被限流时**不进入密码哈希**，既不泄露「密码是否正确」，也不让攻击者白白消耗服务器 CPU。
//!
//! 已知取舍：按账号限流意味着别人可以故意连续输错来**暂时**锁住你的账号（窗口过后自动恢复）。
//! 这比无限次猜密码的风险小，是行业里常见的折中；如果这不可接受，需要引入验证码或 IP 信誉等更重的手段。

use std::net::IpAddr;
use std::time::Duration;

use async_trait::async_trait;

use super::PortFailure;

/// 某个限流键当前的计数状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttemptStatus {
    /// 当前窗口内已记录的次数。
    pub count: u32,
    /// 窗口还有多久结束、计数归零。
    pub resets_in: Duration,
}

/// 尝试次数计数端口：固定窗口计数器。
///
/// `key` 可能含邮箱这类个人信息，实现方不应原样存储或记录它（比如先取哈希）。
#[async_trait]
pub trait AttemptCounter: Send + Sync {
    /// 查询当前状态；窗口内没有记录时返回 `None`。
    async fn status(&self, key: &str) -> Result<Option<AttemptStatus>, PortFailure>;

    /// 次数加一并返回新值。键不存在时创建并开启一个 `window` 长度的窗口；
    /// 键已存在时**不延长**窗口（固定窗口，避免持续尝试把窗口越拖越长）。
    async fn increment(&self, key: &str, window: Duration) -> Result<u32, PortFailure>;

    /// 清零。键不存在时也算成功。
    async fn reset(&self, key: &str) -> Result<(), PortFailure>;
}

/// 限流策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThrottlePolicy {
    /// 同一个 IP 在一个窗口内允许的登录失败次数。
    pub login_max_failures_per_ip: u32,
    /// 同一个账号（不论来自哪个 IP）在一个窗口内允许的登录失败次数。
    pub login_max_failures_per_account: u32,
    /// 登录失败计数的窗口长度。
    pub login_window: Duration,
    /// 同一个 IP 在一个窗口内允许的注册请求次数（成功与否都算，因为哈希本身就是开销）。
    pub register_max_per_ip: u32,
    /// 注册计数的窗口长度。
    pub register_window: Duration,
}

impl Default for ThrottlePolicy {
    fn default() -> Self {
        Self {
            login_max_failures_per_ip: 20,
            login_max_failures_per_account: 10,
            login_window: Duration::from_secs(15 * 60),
            register_max_per_ip: 10,
            register_window: Duration::from_secs(60 * 60),
        }
    }
}

/// 一条具体的限流规则：计数键、上限、窗口。
pub(crate) struct Limit {
    pub(crate) key: String,
    pub(crate) max: u32,
    pub(crate) window: Duration,
}

/// IP 的限流键。IPv6 按 /64 前缀归并：一台机器通常能拿到整个 /64，逐个地址计数形同虚设。
fn ip_key_part(client_ip: IpAddr) -> String {
    match client_ip {
        IpAddr::V4(address) => address.to_string(),
        IpAddr::V6(address) => {
            let segments = address.segments();
            format!(
                "{:x}:{:x}:{:x}:{:x}::/64",
                segments[0], segments[1], segments[2], segments[3]
            )
        }
    }
}

impl ThrottlePolicy {
    /// 登录要检查的规则：有 IP 就按 IP，账号标识合法就按账号。
    ///
    /// `scope` 是限流键的前缀（普通用户域用 `login`，管理员域用 `admin-login`），
    /// 两个域的计数因此互不影响：攻击者刷普通登录的失败次数，不会锁住管理员账号，反过来也一样。
    pub(crate) fn login_limits(
        &self,
        scope: &str,
        client_ip: Option<IpAddr>,
        normalized_account: Option<&str>,
    ) -> Vec<Limit> {
        let mut limits = Vec::with_capacity(2);
        if let Some(client_ip) = client_ip {
            limits.push(Limit {
                key: format!("{scope}:ip:{}", ip_key_part(client_ip)),
                max: self.login_max_failures_per_ip,
                window: self.login_window,
            });
        }
        if let Some(account) = normalized_account {
            limits.push(Limit {
                key: format!("{scope}:account:{account}"),
                max: self.login_max_failures_per_account,
                window: self.login_window,
            });
        }
        limits
    }

    /// 登录成功后要清零的键：只有账号，没有 IP（见模块文档）。
    pub(crate) fn login_account_key(scope: &str, normalized_account: &str) -> String {
        format!("{scope}:account:{normalized_account}")
    }

    /// 注册要检查的规则：只按 IP。
    pub(crate) fn register_limits(&self, client_ip: Option<IpAddr>) -> Vec<Limit> {
        client_ip
            .map(|client_ip| Limit {
                key: format!("register:ip:{}", ip_key_part(client_ip)),
                max: self.register_max_per_ip,
                window: self.register_window,
            })
            .into_iter()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr};

    use super::*;

    #[test]
    fn ipv6_addresses_in_the_same_slash_64_share_a_key() {
        let first: IpAddr = "2001:db8:1:2:aaaa::1".parse().unwrap();
        let second: IpAddr = "2001:db8:1:2:bbbb::ffff".parse().unwrap();
        let other_network: IpAddr = "2001:db8:1:3::1".parse().unwrap();

        assert_eq!(ip_key_part(first), ip_key_part(second));
        assert_ne!(ip_key_part(first), ip_key_part(other_network));
        assert_eq!(ip_key_part(IpAddr::V6(Ipv6Addr::LOCALHOST)), "0:0:0:0::/64");
    }

    #[test]
    fn ipv4_addresses_are_keyed_individually() {
        assert_eq!(
            ip_key_part(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7))),
            "203.0.113.7"
        );
    }

    #[test]
    fn limits_depend_on_what_is_known_about_the_request() {
        let policy = ThrottlePolicy::default();
        let client_ip: IpAddr = "203.0.113.7".parse().unwrap();

        assert_eq!(
            policy
                .login_limits("login", Some(client_ip), Some("a@example.com"))
                .len(),
            2
        );
        assert_eq!(
            policy.login_limits("login", Some(client_ip), None).len(),
            1,
            "邮箱格式不合法时只按 IP"
        );
        assert_eq!(
            policy
                .login_limits("login", None, Some("a@example.com"))
                .len(),
            1,
            "拿不到 IP 时只按账号"
        );
        assert!(policy.login_limits("login", None, None).is_empty());
        assert_eq!(policy.register_limits(Some(client_ip)).len(), 1);
        assert!(policy.register_limits(None).is_empty());
    }

    #[test]
    fn different_scopes_never_share_a_counter_key() {
        let policy = ThrottlePolicy::default();
        let client_ip: IpAddr = "203.0.113.7".parse().unwrap();
        let user_keys: Vec<_> = policy
            .login_limits("login", Some(client_ip), Some("alice"))
            .into_iter()
            .map(|limit| limit.key)
            .collect();
        let admin_keys: Vec<_> = policy
            .login_limits("admin-login", Some(client_ip), Some("alice"))
            .into_iter()
            .map(|limit| limit.key)
            .collect();

        assert_eq!(user_keys, ["login:ip:203.0.113.7", "login:account:alice"]);
        assert_eq!(
            admin_keys,
            ["admin-login:ip:203.0.113.7", "admin-login:account:alice"]
        );
        assert!(user_keys.iter().all(|key| !admin_keys.contains(key)));
    }

    #[test]
    fn login_and_register_use_separate_counters_for_the_same_ip() {
        let policy = ThrottlePolicy::default();
        let client_ip: IpAddr = "203.0.113.7".parse().unwrap();
        let login_key = &policy.login_limits("login", Some(client_ip), None)[0].key;
        let register_key = &policy.register_limits(Some(client_ip))[0].key;
        assert_ne!(login_key, register_key);
    }
}
