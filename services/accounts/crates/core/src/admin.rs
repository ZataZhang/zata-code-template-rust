//! 管理员域：管理员账号的登录与会话，以及管理后台的用户管理用例。
//!
//! 与普通用户域（[`crate::auth`]）**完全隔离**：
//!
//! - 管理员是独立的一张表，登录标识是**用户名**；没有任何注册入口，只能由启动时的引导账号创建；
//! - 会话由独立的会话存储实例保存（键前缀不同），所以一个域签发的令牌在另一个域里永远查不到；
//! - 登录限流使用独立的键前缀（`admin-login`），两个域的失败计数互不影响，且管理员默认的上限更严格；
//! - 没有「用户表里加一个管理员标志」这回事：普通登录或会话被攻破，拿不到任何管理权限。
//!
//! 与普通域共用的只有端口（会话存储、密码哈希、计数器）以及登录辅助（限流器、诱饵哈希），
//! 这些在 [`crate::auth`] 里，避免把安全逻辑复制成两份。

use std::fmt;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::auth::{
    AttemptCounter, AuthError, DECOY_PASSWORD, Limit, MAX_PASSWORD_BYTES, PasswordHasher,
    SessionStore, SessionToken, ThrottlePolicy, Throttler, User, UserId, UserListQuery, UserPage,
    UserRepository, UserStatus, validate_password, verify_with_decoy,
};

#[cfg(any(test, feature = "testing"))]
pub mod testing;

/// 管理员用户名最大字节数。
pub const MAX_USERNAME_BYTES: usize = 64;
/// 用户列表的默认每页条数。
pub const DEFAULT_PAGE_SIZE: u32 = 20;
/// 用户列表每页条数的上限：不设上限就是一个让调用方一次拉走整张表的接口。
pub const MAX_PAGE_SIZE: u32 = 100;
/// 邮箱搜索关键字的最大字符数。
const MAX_KEYWORD_CHARS: usize = 100;
/// 管理员登录限流使用的作用域（限流键前缀）。
const ADMIN_LOGIN_SCOPE: &str = "admin-login";

/// 管理员 ID。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AdminId(pub i64);

/// 对外可见的管理员信息（不含密码哈希）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admin {
    /// 管理员 ID。
    pub id: AdminId,
    /// 规范化（去空白、小写）后的用户名。
    pub username: String,
    /// 显示名。
    pub display_name: String,
}

/// 登录校验用的管理员凭据。只在登录时使用，不应出现在任何响应或日志里。
#[derive(Clone, PartialEq, Eq)]
pub struct AdminCredentials {
    /// 管理员信息。
    pub admin: Admin,
    /// 密码哈希（PHC 字符串格式）。
    pub password_hash: String,
}

impl fmt::Debug for AdminCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdminCredentials")
            .field("admin", &self.admin)
            .field("password_hash", &"<已隐藏>")
            .finish()
    }
}

/// 管理员存储端口的错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AdminRepositoryError {
    /// 用户名已存在。
    #[error("管理员用户名已存在")]
    UsernameTaken,
    /// 存储暂时不可用，原因只用于服务端日志。
    #[error("管理员存储不可用：{0}")]
    Unavailable(String),
}

impl From<AdminRepositoryError> for AuthError {
    fn from(error: AdminRepositoryError) -> Self {
        Self::Internal(error.to_string())
    }
}

/// 管理员存储端口。
#[async_trait]
pub trait AdminRepository: Send + Sync {
    /// 创建管理员。`username` 已规范化；重复时必须返回 [`AdminRepositoryError::UsernameTaken`]。
    async fn create(
        &self,
        username: &str,
        display_name: &str,
        password_hash: &str,
    ) -> Result<Admin, AdminRepositoryError>;

    /// 按用户名查登录凭据；`username` 已规范化。
    async fn find_credentials_by_username(
        &self,
        username: &str,
    ) -> Result<Option<AdminCredentials>, AdminRepositoryError>;

    /// 按 ID 查管理员。
    async fn find_by_id(&self, admin_id: AdminId) -> Result<Option<Admin>, AdminRepositoryError>;
}

/// 去掉首尾空白并转小写，同时检查长度与字符。
///
/// 允许字母、数字和 `. _ - @ +`（所以邮箱形式的用户名也可以）。
///
/// # Errors
///
/// 为空、太长或含不允许的字符时返回 [`AuthError::InvalidUsername`]。
pub fn normalize_username(raw_username: &str) -> Result<String, AuthError> {
    let username = raw_username.trim().to_lowercase();
    let is_valid = !username.is_empty()
        && username.len() <= MAX_USERNAME_BYTES
        && username
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@' | '+'));
    if is_valid {
        Ok(username)
    } else {
        Err(AuthError::InvalidUsername)
    }
}

/// 管理员域依赖的端口实现。
pub struct AdminPorts {
    /// 管理员存储。
    pub admin_repository: Arc<dyn AdminRepository>,
    /// 用户存储（管理后台要列出和启停用户）。
    pub user_repository: Arc<dyn UserRepository>,
    /// 管理员会话存储：必须是独立的实例（键前缀与普通域不同）。
    pub session_store: Arc<dyn SessionStore>,
    /// 密码哈希器。
    pub password_hasher: Arc<dyn PasswordHasher>,
    /// 尝试次数计数器（限流用）。
    pub attempt_counter: Arc<dyn AttemptCounter>,
}

/// 管理员域的策略参数。
#[derive(Debug, Clone, Copy)]
pub struct AdminPolicy {
    /// 管理员会话有效期。
    pub session_ttl: Duration,
    /// 管理员登录的限流策略（只用到登录相关的字段）。
    pub throttle: ThrottlePolicy,
}

/// 引导管理员的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootstrapOutcome {
    /// 新创建了管理员。
    Created(Admin),
    /// 该用户名已存在，什么都没改（尤其不会覆盖已有的密码）。
    AlreadyExists,
}

/// 管理后台列用户的请求：缺省值与上下限在服务里统一处理。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserListRequest {
    /// 页码，从 1 开始；缺省为 1。
    pub page: Option<u32>,
    /// 每页条数；缺省 [`DEFAULT_PAGE_SIZE`]，上限 [`MAX_PAGE_SIZE`]。
    pub page_size: Option<u32>,
    /// 只看某个状态。
    pub status: Option<UserStatus>,
    /// 邮箱关键字（字面子串，不区分大小写）。
    pub keyword: Option<String>,
}

/// 管理员域用例：管理员登录与会话、用户管理。
pub struct AdminService {
    ports: AdminPorts,
    policy: AdminPolicy,
    throttler: Throttler,
    /// 用户名不存在时也要做一次哈希校验用的假哈希。
    decoy_password_hash: String,
}

impl AdminService {
    /// 创建服务。会预先计算一个诱饵哈希，所以是异步的，且应在启动时调用一次。
    ///
    /// # Errors
    ///
    /// 密码哈希器失败时返回 [`AuthError::Internal`]。
    pub async fn new(ports: AdminPorts, policy: AdminPolicy) -> Result<Self, AuthError> {
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

    /// 启动时引导第一个管理员：用户名不存在才创建，已存在则**不改动**（不覆盖密码）。
    ///
    /// 这是创建管理员的**唯一**途径：没有注册入口，也没有任何接口能创建管理员。
    ///
    /// # Errors
    ///
    /// 用户名或密码不合规，或依赖不可用时返回对应错误。
    pub async fn bootstrap_admin(
        &self,
        raw_username: &str,
        password: &str,
    ) -> Result<BootstrapOutcome, AuthError> {
        let username = normalize_username(raw_username)?;
        validate_password(password)?;
        if self
            .ports
            .admin_repository
            .find_credentials_by_username(&username)
            .await?
            .is_some()
        {
            return Ok(BootstrapOutcome::AlreadyExists);
        }
        let password_hash = self.ports.password_hasher.hash(password).await?;
        match self
            .ports
            .admin_repository
            .create(&username, &username, &password_hash)
            .await
        {
            Ok(admin) => Ok(BootstrapOutcome::Created(admin)),
            // 多个实例同时启动时可能同时创建：唯一约束保证只有一个成功，其余视为「已存在」。
            Err(AdminRepositoryError::UsernameTaken) => Ok(BootstrapOutcome::AlreadyExists),
            Err(error) => Err(error.into()),
        }
    }

    /// 管理员登录，成功时返回管理员和新的会话令牌。
    ///
    /// 与普通登录同样的防护：不存在的用户名与密码错误返回同一个错误且耗时相近；失败按 IP 和按账号计数
    /// （不存在的用户名也计数）；达到上限后不进入哈希；成功只清零账号计数。限流键用独立的前缀。
    ///
    /// # Errors
    ///
    /// 凭据不对返回 [`AuthError::InvalidCredentials`]；被限流返回 [`AuthError::TooManyAttempts`]。
    pub async fn login(
        &self,
        raw_username: &str,
        password: &str,
        client_ip: Option<IpAddr>,
    ) -> Result<(Admin, SessionToken), AuthError> {
        let normalized_username = normalize_username(raw_username).ok();
        let limits: Vec<Limit> = self.policy.throttle.login_limits(
            ADMIN_LOGIN_SCOPE,
            client_ip,
            normalized_username.as_deref(),
        );
        self.throttler.ensure_within_limits(&limits).await?;

        let admin = self
            .verify_credentials(normalized_username.as_deref(), password)
            .await?;
        let Some(admin) = admin else {
            self.throttler.record(&limits).await?;
            return Err(AuthError::InvalidCredentials);
        };

        if let Some(username) = normalized_username.as_deref() {
            self.throttler
                .reset(&ThrottlePolicy::login_account_key(
                    ADMIN_LOGIN_SCOPE,
                    username,
                ))
                .await?;
        }
        let session_token = self
            .ports
            .session_store
            .create(admin.id.0, self.policy.session_ttl)
            .await?;
        Ok((admin, session_token))
    }

    async fn verify_credentials(
        &self,
        normalized_username: Option<&str>,
        password: &str,
    ) -> Result<Option<Admin>, AuthError> {
        // 超长密码直接判失败，不进入昂贵的哈希计算。
        if password.len() > MAX_PASSWORD_BYTES {
            return Ok(None);
        }
        let credentials = match normalized_username {
            Some(username) => {
                self.ports
                    .admin_repository
                    .find_credentials_by_username(username)
                    .await?
            }
            None => None,
        };
        let matches = verify_with_decoy(
            self.ports.password_hasher.as_ref(),
            &self.decoy_password_hash,
            credentials
                .as_ref()
                .map(|found| found.password_hash.as_str()),
            password,
        )
        .await?;
        Ok(credentials.filter(|_| matches).map(|found| found.admin))
    }

    /// 根据管理员会话令牌找出当前管理员。
    ///
    /// # Errors
    ///
    /// 令牌无效或已过期（或管理员已不存在）返回 [`AuthError::Unauthenticated`]。
    pub async fn authenticate(&self, session_token: &SessionToken) -> Result<Admin, AuthError> {
        let Some(principal_id) = self.ports.session_store.resolve(session_token).await? else {
            return Err(AuthError::Unauthenticated);
        };
        self.ports
            .admin_repository
            .find_by_id(AdminId(principal_id))
            .await?
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

    /// 分页列出用户。页码、每页条数、关键字在这里统一规整，调用方（HTTP 层）不必各自处理。
    ///
    /// # Errors
    ///
    /// 用户存储不可用时返回 [`AuthError::Internal`]。
    pub async fn list_users(&self, request: UserListRequest) -> Result<UserPage, AuthError> {
        let keyword = request
            .keyword
            .map(|keyword| {
                keyword
                    .trim()
                    .chars()
                    .take(MAX_KEYWORD_CHARS)
                    .collect::<String>()
            })
            .filter(|keyword| !keyword.is_empty());
        let query = UserListQuery {
            page: request.page.unwrap_or(1).max(1),
            page_size: request
                .page_size
                .unwrap_or(DEFAULT_PAGE_SIZE)
                .clamp(1, MAX_PAGE_SIZE),
            status: request.status,
            keyword,
        };
        Ok(self.ports.user_repository.list(&query).await?)
    }

    /// 按 ID 查一个用户。
    ///
    /// # Errors
    ///
    /// 用户存储不可用时返回 [`AuthError::Internal`]。
    pub async fn get_user(&self, user_id: i64) -> Result<Option<User>, AuthError> {
        Ok(self
            .ports
            .user_repository
            .find_by_id(UserId(user_id))
            .await?)
    }

    /// 启用或停用用户，返回修改后的用户；用户不存在返回 `None`。重复设置同一状态是幂等的。
    ///
    /// 停用**立即生效**：普通域每次校验会话都会读取用户当前状态，所以不需要撤销该用户的会话。
    ///
    /// # Errors
    ///
    /// 用户存储不可用时返回 [`AuthError::Internal`]。
    pub async fn set_user_status(
        &self,
        user_id: i64,
        status: UserStatus,
    ) -> Result<Option<User>, AuthError> {
        Ok(self
            .ports
            .user_repository
            .set_status(UserId(user_id), status)
            .await?)
    }
}

#[cfg(test)]
mod tests;
