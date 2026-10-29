//! 管理员仓储：PostgreSQL 实现。

use async_trait::async_trait;
use sqlx::PgPool;
use tpl_core::admin::{Admin, AdminCredentials, AdminId, AdminRepository, AdminRepositoryError};

/// 管理员表的一行（不含密码哈希）。
struct AdminRow {
    id: i64,
    username: String,
    display_name: String,
}

/// 管理员表的一行（含密码哈希，仅登录校验使用）。
struct CredentialsRow {
    id: i64,
    username: String,
    display_name: String,
    password_hash: String,
}

impl From<AdminRow> for Admin {
    fn from(row: AdminRow) -> Self {
        Self {
            id: AdminId(row.id),
            username: row.username,
            display_name: row.display_name,
        }
    }
}

/// 把 sqlx 错误转成端口错误。唯一约束冲突说明用户名已存在。
fn to_repository_error(query_error: sqlx::Error) -> AdminRepositoryError {
    match &query_error {
        sqlx::Error::Database(database_error) if database_error.is_unique_violation() => {
            AdminRepositoryError::UsernameTaken
        }
        _ => AdminRepositoryError::Unavailable(query_error.to_string()),
    }
}

/// 基于 PostgreSQL 的管理员仓储。查询在编译期校验，所有外部值都用参数绑定。
pub struct PostgresAdminRepository {
    connection_pool: PgPool,
}

impl PostgresAdminRepository {
    /// 基于连接池创建仓储。
    pub fn new(connection_pool: PgPool) -> Self {
        Self { connection_pool }
    }
}

#[async_trait]
impl AdminRepository for PostgresAdminRepository {
    async fn create(
        &self,
        username: &str,
        display_name: &str,
        password_hash: &str,
    ) -> Result<Admin, AdminRepositoryError> {
        sqlx::query_as!(
            AdminRow,
            "INSERT INTO admin_users (username, display_name, password_hash) \
             VALUES ($1, $2, $3) RETURNING id, username, display_name",
            username,
            display_name,
            password_hash
        )
        .fetch_one(&self.connection_pool)
        .await
        .map(Admin::from)
        .map_err(to_repository_error)
    }

    async fn find_credentials_by_username(
        &self,
        username: &str,
    ) -> Result<Option<AdminCredentials>, AdminRepositoryError> {
        sqlx::query_as!(
            CredentialsRow,
            "SELECT id, username, display_name, password_hash FROM admin_users WHERE username = $1",
            username
        )
        .fetch_optional(&self.connection_pool)
        .await
        .map(|row| {
            row.map(|credentials_row| AdminCredentials {
                admin: Admin::from(AdminRow {
                    id: credentials_row.id,
                    username: credentials_row.username,
                    display_name: credentials_row.display_name,
                }),
                password_hash: credentials_row.password_hash,
            })
        })
        .map_err(to_repository_error)
    }

    async fn find_by_id(&self, admin_id: AdminId) -> Result<Option<Admin>, AdminRepositoryError> {
        sqlx::query_as!(
            AdminRow,
            "SELECT id, username, display_name FROM admin_users WHERE id = $1",
            admin_id.0
        )
        .fetch_optional(&self.connection_pool)
        .await
        .map(|row| row.map(Admin::from))
        .map_err(to_repository_error)
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
        run_migrations(&connection_pool).await.unwrap();
        let repository = PostgresAdminRepository::new(connection_pool.clone());
        let unique_suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let username = format!("repo-test-{unique_suffix}");

        let created = repository
            .create(&username, "显示名", "hash-1")
            .await
            .unwrap();
        assert_eq!(
            (created.username.as_str(), created.display_name.as_str()),
            (username.as_str(), "显示名")
        );

        let credentials = repository
            .find_credentials_by_username(&username)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(credentials.admin, created);
        assert_eq!(credentials.password_hash, "hash-1");
        assert_eq!(
            repository.find_by_id(created.id).await.unwrap(),
            Some(created.clone())
        );
        assert_eq!(
            repository
                .find_credentials_by_username("nobody-here")
                .await
                .unwrap(),
            None
        );

        assert_eq!(
            repository.create(&username, "x", "hash-2").await,
            Err(AdminRepositoryError::UsernameTaken)
        );
        let uppercase_insert = sqlx::query(
            "INSERT INTO admin_users (username, display_name, password_hash) VALUES ($1, 'x', 'x')",
        )
        .bind(username.to_uppercase())
        .execute(&connection_pool)
        .await;
        assert!(uppercase_insert.is_err(), "大写用户名应被 CHECK 约束拒绝");

        sqlx::query("DELETE FROM admin_users WHERE username = $1")
            .bind(&username)
            .execute(&connection_pool)
            .await
            .unwrap();
    }
}
