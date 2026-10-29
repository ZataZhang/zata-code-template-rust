//! 用户仓储：PostgreSQL 实现。

use async_trait::async_trait;
use sqlx::PgPool;
use tpl_core::auth::{
    StoredCredentials, User, UserId, UserListQuery, UserPage, UserRepository, UserRepositoryError,
    UserStatus,
};

/// 用户表的一行（不含密码哈希）。
struct UserRow {
    id: i64,
    email: String,
    status: String,
    created_at: String,
}

/// 用户表的一行（含密码哈希，仅登录校验使用）。
struct CredentialsRow {
    id: i64,
    email: String,
    status: String,
    created_at: String,
    password_hash: String,
}

impl TryFrom<UserRow> for User {
    type Error = UserRepositoryError;

    /// 状态列有 CHECK 约束；万一读到不认识的取值，报错而不是悄悄当成「启用」。
    fn try_from(row: UserRow) -> Result<Self, Self::Error> {
        let status = UserStatus::parse(&row.status).ok_or_else(|| {
            UserRepositoryError::Unavailable(format!("用户 {} 的状态取值无法识别", row.id))
        })?;
        Ok(Self {
            id: UserId(row.id),
            email: row.email,
            status,
            created_at: row.created_at,
        })
    }
}

/// 转义 `LIKE` 的通配符，让关键字按字面匹配（否则 `%`、`_` 会变成「匹配一切」）。
fn escape_like(keyword: &str) -> String {
    let mut escaped = String::with_capacity(keyword.len());
    for character in keyword.chars() {
        if matches!(character, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

/// 把 sqlx 错误转成端口错误。唯一约束冲突说明邮箱已被注册。
fn to_repository_error(query_error: sqlx::Error) -> UserRepositoryError {
    match &query_error {
        sqlx::Error::Database(database_error) if database_error.is_unique_violation() => {
            UserRepositoryError::EmailTaken
        }
        _ => UserRepositoryError::Unavailable(query_error.to_string()),
    }
}

/// 基于 PostgreSQL 的用户仓储。查询在编译期校验，所有外部值都用参数绑定。
pub struct PostgresUserRepository {
    connection_pool: PgPool,
}

impl PostgresUserRepository {
    /// 基于连接池创建仓储。
    pub fn new(connection_pool: PgPool) -> Self {
        Self { connection_pool }
    }
}

#[async_trait]
impl UserRepository for PostgresUserRepository {
    async fn create(&self, email: &str, password_hash: &str) -> Result<User, UserRepositoryError> {
        let row = sqlx::query_as!(
            UserRow,
            r#"INSERT INTO users (email, password_hash) VALUES ($1, $2)
               RETURNING id, email, status,
                 to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS "created_at!""#,
            email,
            password_hash
        )
        .fetch_one(&self.connection_pool)
        .await
        .map_err(to_repository_error)?;
        User::try_from(row)
    }

    async fn find_credentials_by_email(
        &self,
        email: &str,
    ) -> Result<Option<StoredCredentials>, UserRepositoryError> {
        let row = sqlx::query_as!(
            CredentialsRow,
            r#"SELECT id, email, status,
                 to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS "created_at!",
                 password_hash FROM users WHERE email = $1"#,
            email
        )
        .fetch_optional(&self.connection_pool)
        .await
        .map_err(to_repository_error)?;
        row.map(|credentials_row| {
            Ok(StoredCredentials {
                user: User::try_from(UserRow {
                    id: credentials_row.id,
                    email: credentials_row.email,
                    status: credentials_row.status,
                    created_at: credentials_row.created_at,
                })?,
                password_hash: credentials_row.password_hash,
            })
        })
        .transpose()
    }

    async fn find_by_id(&self, user_id: UserId) -> Result<Option<User>, UserRepositoryError> {
        let row = sqlx::query_as!(
            UserRow,
            r#"SELECT id, email, status,
                 to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS "created_at!"
               FROM users WHERE id = $1"#,
            user_id.0
        )
        .fetch_optional(&self.connection_pool)
        .await
        .map_err(to_repository_error)?;
        row.map(User::try_from).transpose()
    }

    async fn list(&self, query: &UserListQuery) -> Result<UserPage, UserRepositoryError> {
        // SQLx 宏要求查询是字面量，因此总数与分页列表各自写出相同的固定过滤条件。
        let status = query.status.map(UserStatus::as_str);
        let keyword = query.keyword.as_deref().map(escape_like);

        let total = sqlx::query_scalar!(
            r#"SELECT count(*) AS "count!" FROM users
               WHERE ($1::text IS NULL OR status = $1)
                 AND ($2::text IS NULL OR email ILIKE '%' || $2 || '%' ESCAPE '\')"#,
            status,
            keyword.as_deref()
        )
        .fetch_one(&self.connection_pool)
        .await
        .map_err(to_repository_error)?;
        let offset = i64::from(query.page.saturating_sub(1)) * i64::from(query.page_size);
        let rows = sqlx::query_as!(
            UserRow,
            r#"SELECT id, email, status,
                 to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS "created_at!"
               FROM users
               WHERE ($1::text IS NULL OR status = $1)
                 AND ($2::text IS NULL OR email ILIKE '%' || $2 || '%' ESCAPE '\')
               ORDER BY id LIMIT $3 OFFSET $4"#,
            status,
            keyword.as_deref(),
            i64::from(query.page_size),
            offset
        )
        .fetch_all(&self.connection_pool)
        .await
        .map_err(to_repository_error)?;

        Ok(UserPage {
            items: rows
                .into_iter()
                .map(User::try_from)
                .collect::<Result<_, _>>()?,
            total: u64::try_from(total).unwrap_or(0),
        })
    }

    async fn set_status(
        &self,
        user_id: UserId,
        status: UserStatus,
    ) -> Result<Option<User>, UserRepositoryError> {
        let row = sqlx::query_as!(
            UserRow,
            r#"UPDATE users SET status = $2 WHERE id = $1
               RETURNING id, email, status,
                 to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS "created_at!""#,
            user_id.0,
            status.as_str()
        )
        .fetch_optional(&self.connection_pool)
        .await
        .map_err(to_repository_error)?;
        row.map(User::try_from).transpose()
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::config::DatabaseSettings;
    use crate::persistence::{connect_lazy, run_migrations};

    /// 真实数据库测试：默认跳过，用 `just test real` 显式运行。
    #[tokio::test]
    #[ignore = "需要真实 PostgreSQL，设置 TEST_DATABASE_URL 后用 `just test real` 运行"]
    async fn create_find_and_reject_duplicates_against_a_real_postgres() {
        let database_url = std::env::var("TEST_DATABASE_URL").expect("请设置 TEST_DATABASE_URL");
        let connection_pool =
            connect_lazy(&DatabaseSettings::default(), &database_url).expect("连接串应当可解析");
        run_migrations(&connection_pool)
            .await
            .expect("迁移应能成功（且可重复执行）");
        let user_repository = PostgresUserRepository::new(connection_pool.clone());

        // 每次运行用不同的邮箱，测试结束再按精确邮箱删除，不留残留。
        let unique_suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let email = format!("repo-test-{unique_suffix}@example.test");

        let created_user = user_repository.create(&email, "hash-1").await.unwrap();
        assert_eq!(created_user.email, email);

        let found_credentials = user_repository
            .find_credentials_by_email(&email)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found_credentials.user, created_user);
        assert_eq!(found_credentials.password_hash, "hash-1");
        assert_eq!(
            user_repository.find_by_id(created_user.id).await.unwrap(),
            Some(created_user.clone())
        );
        assert_eq!(
            user_repository
                .find_credentials_by_email("nobody-here@example.test")
                .await
                .unwrap(),
            None
        );

        // 唯一约束：同一邮箱再次创建必须是 EmailTaken，而不是笼统的"不可用"。
        assert_eq!(
            user_repository.create(&email, "hash-2").await,
            Err(UserRepositoryError::EmailTaken)
        );
        // 兜底约束：绕过应用层直接写大写邮箱会被 CHECK 拒绝。
        let uppercase_insert =
            sqlx::query("INSERT INTO users (email, password_hash) VALUES ($1, 'x')")
                .bind(email.to_uppercase())
                .execute(&connection_pool)
                .await;
        assert!(
            uppercase_insert.is_err(),
            "大写邮箱应被 users_email_lowercase 约束拒绝"
        );

        sqlx::query("DELETE FROM users WHERE email = $1")
            .bind(&email)
            .execute(&connection_pool)
            .await
            .unwrap();
    }

    /// 列表、过滤、分页、关键字转义与启停，都对着真实 PostgreSQL 验证。
    #[tokio::test]
    #[ignore = "需要真实 PostgreSQL，设置 TEST_DATABASE_URL 后用 `just test real` 运行"]
    async fn list_filter_paginate_and_toggle_status_against_a_real_postgres() {
        let database_url = std::env::var("TEST_DATABASE_URL").expect("请设置 TEST_DATABASE_URL");
        let connection_pool =
            connect_lazy(&DatabaseSettings::default(), &database_url).expect("连接串应当可解析");
        run_migrations(&connection_pool).await.unwrap();
        let user_repository = PostgresUserRepository::new(connection_pool.clone());

        // 用本次运行独有的关键字，库里有别的数据也不影响断言。
        let tag = format!(
            "lt{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let mut created = Vec::new();
        for name in ["a", "b", "c"] {
            created.push(
                user_repository
                    .create(&format!("{name}-{tag}@example.test"), "h")
                    .await
                    .unwrap(),
            );
        }
        // 含 LIKE 通配符的邮箱：`%`、`_` 必须按字面匹配。
        let wildcard = user_repository
            .create(&format!("50%_off-{tag}@example.test"), "h")
            .await
            .unwrap();

        let query = |keyword: &str, page, page_size, status| UserListQuery {
            page,
            page_size,
            status,
            keyword: Some(keyword.to_string()),
        };

        assert_eq!(created[0].status, UserStatus::Active, "新用户默认启用");
        assert!(
            created[0].created_at.ends_with('Z') && created[0].created_at.contains('T'),
            "创建时间应是 UTC ISO 格式：{}",
            created[0].created_at
        );

        let first = user_repository
            .list(&query(&tag, 1, 3, None))
            .await
            .unwrap();
        let second = user_repository
            .list(&query(&tag, 2, 3, None))
            .await
            .unwrap();
        assert_eq!(
            (first.total, first.items.len(), second.items.len()),
            (4, 3, 1)
        );
        assert_eq!(first.items[0], created[0], "按 ID 升序");

        // 关键字不区分大小写，且通配符按字面匹配。
        let upper = user_repository
            .list(&query(&tag.to_uppercase(), 1, 10, None))
            .await
            .unwrap();
        assert_eq!(upper.total, 4);
        let literal = user_repository
            .list(&query(&format!("50%_off-{tag}"), 1, 10, None))
            .await
            .unwrap();
        assert_eq!(literal.items, vec![wildcard.clone()]);
        let not_a_wildcard = user_repository
            .list(&query(&format!("%-{tag}"), 1, 10, None))
            .await
            .unwrap();
        assert_eq!(not_a_wildcard.total, 0, "`%` 不能当作匹配一切的通配符");

        // 启停与状态过滤。
        let disabled = user_repository
            .set_status(created[1].id, UserStatus::Disabled)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(disabled.status, UserStatus::Disabled);
        let only_disabled = user_repository
            .list(&query(&tag, 1, 10, Some(UserStatus::Disabled)))
            .await
            .unwrap();
        assert_eq!(only_disabled.items, vec![disabled.clone()]);
        assert_eq!(
            user_repository.find_by_id(created[1].id).await.unwrap(),
            Some(disabled)
        );
        let restored = user_repository
            .set_status(created[1].id, UserStatus::Active)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(restored.status, UserStatus::Active);
        assert_eq!(
            user_repository
                .set_status(UserId(i64::MAX), UserStatus::Disabled)
                .await
                .unwrap(),
            None
        );
        // 兜底约束：绕过应用层写入不认识的状态会被 CHECK 拒绝。
        let invalid = sqlx::query("UPDATE users SET status = 'banned' WHERE id = $1")
            .bind(created[0].id.0)
            .execute(&connection_pool)
            .await;
        assert!(invalid.is_err(), "非法状态应被 users_status_valid 约束拒绝");

        sqlx::query("DELETE FROM users WHERE email LIKE $1")
            .bind(format!("%{tag}@example.test"))
            .execute(&connection_pool)
            .await
            .unwrap();
    }
}
