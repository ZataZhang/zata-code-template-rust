//! 认证用例：注册、登录、会话校验、退出。
//!
//! 本模块只含领域规则与**端口**（trait）：用户存储、会话存储、密码哈希都由外层实现，
//! 由 `tpl-accounts` 装配。这样规则可以用内存替身（`testing` 模块，仅测试或启用 `testing` feature 时编译）快速测试，不连任何外部服务。

use std::fmt;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

mod credentials;
mod throttle;

pub(crate) use credentials::{Throttler, verify_with_decoy};
pub(crate) use throttle::Limit;
pub use throttle::{AttemptCounter, AttemptStatus, ThrottlePolicy};

// 内存替身只在测试或显式启用 `testing` feature 时编译，避免明文哈希器之类的东西进入生产构建。
#[cfg(any(test, feature = "testing"))]
pub mod testing;

/// 密码最少字符数。
pub const MIN_PASSWORD_CHARS: usize = 8;
/// 密码最大字节数。哈希是刻意设计得很耗 CPU 的，不设上限就是现成的拒绝服务入口。
pub const MAX_PASSWORD_BYTES: usize = 128;
/// 邮箱最大字节数（RFC 5321 的上限）。
const MAX_EMAIL_BYTES: usize = 254;
/// 用来生成「诱饵哈希」的固定口令，见 [`AuthService::login`]。
pub(crate) const DECOY_PASSWORD: &str = "decoy-password-used-only-to-equalise-login-timing";

/// 用户 ID。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UserId(pub i64);

/// 用户账号状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserStatus {
    /// 正常，可以登录。
    Active,
    /// 已停用：不能登录，已有登录态在下一次校验时失效。数据不删除，随时可以重新启用。
    Disabled,
}

impl UserStatus {
    /// 存储与接口里使用的取值。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disabled => "disabled",
        }
    }

    /// 从存储或接口取值解析；不认识的取值返回 `None`（调用方应当报错，而不是悄悄当成启用）。
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "active" => Some(Self::Active),
            "disabled" => Some(Self::Disabled),
            _ => None,
        }
    }
}

/// 对外可见的用户信息（不含密码哈希）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    /// 用户 ID。
    pub id: UserId,
    /// 规范化（去空白、小写）后的邮箱。
    pub email: String,
    /// 账号状态。
    pub status: UserStatus,
    /// 注册时间（UTC，`YYYY-MM-DDTHH:MM:SSZ`）。
    pub created_at: String,
}

/// 管理后台列用户时的查询条件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserListQuery {
    /// 页码，从 1 开始。
    pub page: u32,
    /// 每页条数。
    pub page_size: u32,
    /// 只看某个状态；`None` 表示全部。
    pub status: Option<UserStatus>,
    /// 邮箱关键字（不区分大小写的子串匹配）；`None` 表示不过滤。
    pub keyword: Option<String>,
}

/// 一页用户及总数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserPage {
    /// 本页的用户，按注册先后（ID 升序）排列。
    pub items: Vec<User>,
    /// 满足条件的总数（不是本页条数）。
    pub total: u64,
}

/// 登录校验用的凭据：用户信息加密码哈希。只在登录时使用，不应出现在任何响应或日志里。
#[derive(Clone, PartialEq, Eq)]
pub struct StoredCredentials {
    /// 用户信息。
    pub user: User,
    /// 密码哈希（PHC 字符串格式）。
    pub password_hash: String,
}

impl fmt::Debug for StoredCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoredCredentials")
            .field("user", &self.user)
            .field("password_hash", &"<已隐藏>")
            .finish()
    }
}

/// 会话令牌：持有它就等于登录态，所以 `Debug` 不打印内容。
#[derive(Clone, PartialEq, Eq)]
pub struct SessionToken(String);

impl SessionToken {
    /// 包装一个令牌字符串。
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// 取出令牌内容（用于写入 Cookie 或查询会话存储）。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SessionToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SessionToken(<已隐藏>)")
    }
}

/// 端口实现（会话存储、密码哈希器）报告的失败，只用于服务端日志。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct PortFailure(pub String);

/// 用户存储端口的错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UserRepositoryError {
    /// 邮箱已被注册。
    #[error("邮箱已被注册")]
    EmailTaken,
    /// 存储暂时不可用，原因只用于服务端日志。
    #[error("用户存储不可用：{0}")]
    Unavailable(String),
}

/// 用户存储端口。
#[async_trait]
pub trait UserRepository: Send + Sync {
    /// 创建用户。`email` 已规范化；邮箱重复时必须返回 [`UserRepositoryError::EmailTaken`]。
    async fn create(&self, email: &str, password_hash: &str) -> Result<User, UserRepositoryError>;

    /// 按邮箱查登录凭据；`email` 已规范化。
    async fn find_credentials_by_email(
        &self,
        email: &str,
    ) -> Result<Option<StoredCredentials>, UserRepositoryError>;

    /// 按 ID 查用户。
    async fn find_by_id(&self, user_id: UserId) -> Result<Option<User>, UserRepositoryError>;

    /// 分页列出用户（管理后台用）。
    async fn list(&self, query: &UserListQuery) -> Result<UserPage, UserRepositoryError>;

    /// 修改用户状态，返回修改后的用户；用户不存在返回 `None`。重复设置同一状态是幂等的。
    async fn set_status(
        &self,
        user_id: UserId,
        status: UserStatus,
    ) -> Result<Option<User>, UserRepositoryError>;
}

/// 会话存储端口。
///
/// 会话属于哪个「主体」由使用它的服务解释：普通用户域存用户 ID，管理员域存管理员 ID，所以这里用中立的
/// `i64`。两个域各自使用**独立的存储实例**（实现方用不同的键前缀隔离），因此一个域签发的令牌在另一个域里
/// 永远查不到。
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// 为主体创建会话，`ttl` 后自动失效。令牌必须是不可猜测的随机值。
    async fn create(&self, principal_id: i64, ttl: Duration) -> Result<SessionToken, PortFailure>;

    /// 查令牌对应的主体 ID；不存在或已过期返回 `None`。
    async fn resolve(&self, token: &SessionToken) -> Result<Option<i64>, PortFailure>;

    /// 使令牌失效。令牌本就不存在时也算成功（幂等）。
    async fn revoke(&self, token: &SessionToken) -> Result<(), PortFailure>;
}

/// 密码哈希端口。实现方要保证不阻塞异步运行时（哈希是 CPU 密集操作）。
#[async_trait]
pub trait PasswordHasher: Send + Sync {
    /// 为密码生成带随机盐的哈希（PHC 字符串）。
    async fn hash(&self, password: &str) -> Result<String, PortFailure>;

    /// 校验密码与哈希是否匹配。哈希格式损坏时返回错误，而不是 `false`。
    async fn verify(&self, password: &str, stored_hash: &str) -> Result<bool, PortFailure>;
}

/// 认证用例的错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    /// 邮箱格式不合法。
    #[error("邮箱格式不正确")]
    InvalidEmail,
    /// 密码太短。
    #[error("密码至少需要 {MIN_PASSWORD_CHARS} 个字符")]
    PasswordTooShort,
    /// 密码太长。
    #[error("密码不能超过 {MAX_PASSWORD_BYTES} 个字节")]
    PasswordTooLong,
    /// 邮箱已被注册。
    #[error("该邮箱已被注册")]
    EmailTaken,
    /// 邮箱或密码错误。刻意不区分「邮箱不存在」和「密码错」。
    #[error("邮箱或密码错误")]
    InvalidCredentials,
    /// 没有登录，或会话已失效。
    #[error("未登录或登录已过期")]
    Unauthenticated,
    /// 账号已被停用。只在**密码正确**之后才会返回，所以不会让别人借此探测哪些邮箱存在。
    #[error("账号已停用")]
    AccountDisabled,
    /// 管理员用户名格式不合法。
    #[error("用户名格式不正确")]
    InvalidUsername,
    /// 尝试次数过多，被限流了。`retry_after` 是还需要等多久。
    #[error("尝试次数过多，请稍后再试")]
    TooManyAttempts {
        /// 限流窗口还剩多久结束。
        retry_after: Duration,
    },
    /// 内部错误，原因只用于服务端日志，不应展示给调用方。
    #[error("内部错误：{0}")]
    Internal(String),
}

impl From<PortFailure> for AuthError {
    fn from(failure: PortFailure) -> Self {
        Self::Internal(failure.0)
    }
}

impl From<UserRepositoryError> for AuthError {
    fn from(error: UserRepositoryError) -> Self {
        match error {
            UserRepositoryError::EmailTaken => Self::EmailTaken,
            UserRepositoryError::Unavailable(reason) => Self::Internal(reason),
        }
    }
}

/// 去掉首尾空白并转小写，同时做基本的格式检查。
///
/// 只做「看起来像邮箱」的检查，不追求覆盖 RFC 5322 的全部语法；真正确认邮箱归属要靠验证邮件。
///
/// # Errors
///
/// 格式不合法时返回 [`AuthError::InvalidEmail`]。
pub fn normalize_email(raw_email: &str) -> Result<String, AuthError> {
    let email = raw_email.trim().to_lowercase();
    let looks_like_email = email.len() <= MAX_EMAIL_BYTES
        && !email
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
        && email.split_once('@').is_some_and(|(local_part, domain)| {
            !local_part.is_empty()
                && !domain.contains('@')
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
        });
    if looks_like_email {
        Ok(email)
    } else {
        Err(AuthError::InvalidEmail)
    }
}

/// 检查密码长度规则。
///
/// # Errors
///
/// 太短或太长时返回对应的 [`AuthError`]。
pub fn validate_password(password: &str) -> Result<(), AuthError> {
    if password.len() > MAX_PASSWORD_BYTES {
        return Err(AuthError::PasswordTooLong);
    }
    if password.chars().count() < MIN_PASSWORD_CHARS {
        return Err(AuthError::PasswordTooShort);
    }
    Ok(())
}

/// 普通用户登录限流使用的作用域（限流键前缀）。管理员域用另一个，两边的计数互不影响。
const LOGIN_SCOPE: &str = "login";

/// 认证用例依赖的端口实现。
pub struct AuthPorts {
    /// 用户存储。
    pub user_repository: Arc<dyn UserRepository>,
    /// 会话存储。
    pub session_store: Arc<dyn SessionStore>,
    /// 密码哈希器。
    pub password_hasher: Arc<dyn PasswordHasher>,
    /// 尝试次数计数器（限流用）。
    pub attempt_counter: Arc<dyn AttemptCounter>,
}

/// 认证用例的策略参数。
#[derive(Debug, Clone, Copy)]
pub struct AuthPolicy {
    /// 会话有效期。
    pub session_ttl: Duration,
    /// 登录与注册的限流策略。
    pub throttle: ThrottlePolicy,
}

/// 认证用例。
pub struct AuthService {
    ports: AuthPorts,
    policy: AuthPolicy,
    throttler: Throttler,
    /// 登录时「用户不存在」也要做一次哈希校验用的假哈希，见 [`AuthService::login`]。
    decoy_password_hash: String,
}

impl AuthService {
    /// 创建服务。会预先计算一个诱饵哈希，所以是异步的，且应在启动时调用一次。
    ///
    /// # Errors
    ///
    /// 密码哈希器失败时返回 [`AuthError::Internal`]。
    pub async fn new(ports: AuthPorts, policy: AuthPolicy) -> Result<Self, AuthError> {
        let decoy_password_hash = ports.password_hasher.hash(DECOY_PASSWORD).await?;
        let throttler = Throttler::new(ports.attempt_counter.clone());
        Ok(Self {
            ports,
            policy,
            throttler,
            decoy_password_hash,
        })
    }

    /// 会话有效期，API 层据此设置 Cookie 的 `Max-Age`。
    pub fn session_ttl(&self) -> Duration {
        self.policy.session_ttl
    }

    /// 注册新用户。不会自动登录。
    ///
    /// `client_ip` 用于按 IP 限流；拿不到时传 `None`（则不做按 IP 的限制）。
    /// 每次请求都计数（不论成败），因为哈希本身就是开销。
    ///
    /// # Errors
    ///
    /// 邮箱或密码不合规、邮箱已被注册、被限流、依赖不可用时返回对应错误。
    pub async fn register(
        &self,
        raw_email: &str,
        password: &str,
        client_ip: Option<IpAddr>,
    ) -> Result<User, AuthError> {
        let limits = self.policy.throttle.register_limits(client_ip);
        self.throttler.ensure_within_limits(&limits).await?;
        // 先计数再干活：并发请求也无法绕过上限。
        self.throttler.record(&limits).await?;

        let email = normalize_email(raw_email)?;
        validate_password(password)?;
        let password_hash = self.ports.password_hasher.hash(password).await?;
        // 邮箱唯一性由存储层的唯一约束保证，这样并发注册同一邮箱时也只有一个成功。
        Ok(self
            .ports
            .user_repository
            .create(&email, &password_hash)
            .await?)
    }

    /// 登录，成功时返回用户和新的会话令牌。
    ///
    /// - 「邮箱不存在」和「密码错误」返回同一个错误，且耗时相近：用户不存在时，
    ///   也会拿密码去校验一个诱饵哈希，让攻击者无法靠响应时间枚举哪些邮箱已注册；
    /// - 失败会同时按 IP 和按账号计数（**不存在的邮箱也计数**，不泄露账号是否存在）；
    ///   达到上限后直接拒绝，不进入哈希，也不告诉调用方密码是否正确；
    /// - 成功只清零账号计数，不清零 IP 计数（见 [`ThrottlePolicy`] 的模块说明）。
    ///
    /// # Errors
    ///
    /// 凭据不对返回 [`AuthError::InvalidCredentials`]；被限流返回 [`AuthError::TooManyAttempts`]；
    /// 依赖不可用返回 [`AuthError::Internal`]。
    pub async fn login(
        &self,
        raw_email: &str,
        password: &str,
        client_ip: Option<IpAddr>,
    ) -> Result<(User, SessionToken), AuthError> {
        let normalized_email = normalize_email(raw_email).ok();
        let limits =
            self.policy
                .throttle
                .login_limits(LOGIN_SCOPE, client_ip, normalized_email.as_deref());
        self.throttler.ensure_within_limits(&limits).await?;

        let outcome = self
            .verify_credentials(normalized_email.as_deref(), password)
            .await?;
        let Some(user) = outcome else {
            self.throttler.record(&limits).await?;
            return Err(AuthError::InvalidCredentials);
        };

        // 密码正确，但账号被停用：不签发会话。只在密码正确之后才说「已停用」，不让它变成探测邮箱的手段。
        if user.status == UserStatus::Disabled {
            return Err(AuthError::AccountDisabled);
        }

        if let Some(email) = normalized_email.as_deref() {
            self.throttler
                .reset(&ThrottlePolicy::login_account_key(LOGIN_SCOPE, email))
                .await?;
        }
        let session_token = self
            .ports
            .session_store
            .create(user.id.0, self.policy.session_ttl)
            .await?;
        Ok((user, session_token))
    }

    /// 校验邮箱和密码，匹配返回用户，否则返回 `None`。无论邮箱是否存在都会执行一次哈希校验。
    async fn verify_credentials(
        &self,
        normalized_email: Option<&str>,
        password: &str,
    ) -> Result<Option<User>, AuthError> {
        // 超长密码直接判失败，不进入昂贵的哈希计算。
        if password.len() > MAX_PASSWORD_BYTES {
            return Ok(None);
        }
        let stored_credentials = match normalized_email {
            Some(email) => {
                self.ports
                    .user_repository
                    .find_credentials_by_email(email)
                    .await?
            }
            None => None,
        };
        let password_matches = verify_with_decoy(
            self.ports.password_hasher.as_ref(),
            &self.decoy_password_hash,
            stored_credentials
                .as_ref()
                .map(|found| found.password_hash.as_str()),
            password,
        )
        .await?;
        Ok(stored_credentials
            .filter(|_| password_matches)
            .map(|found| found.user))
    }

    /// 根据会话令牌找出当前用户。
    ///
    /// # Errors
    ///
    /// 令牌无效或已过期（或用户已不存在、已被停用）返回 [`AuthError::Unauthenticated`]。
    pub async fn authenticate(&self, session_token: &SessionToken) -> Result<User, AuthError> {
        let Some(principal_id) = self.ports.session_store.resolve(session_token).await? else {
            return Err(AuthError::Unauthenticated);
        };
        // 每次校验都读用户当前的状态：停用立即生效，不需要去撤销该用户的所有会话。
        self.ports
            .user_repository
            .find_by_id(UserId(principal_id))
            .await?
            .filter(|user| user.status == UserStatus::Active)
            .ok_or(AuthError::Unauthenticated)
    }

    /// 退出登录：使令牌失效。令牌本就无效时也算成功（幂等）。
    ///
    /// # Errors
    ///
    /// 会话存储不可用时返回 [`AuthError::Internal`]。
    pub async fn logout(&self, session_token: &SessionToken) -> Result<(), AuthError> {
        Ok(self.ports.session_store.revoke(session_token).await?)
    }
}

#[cfg(test)]
mod tests;
