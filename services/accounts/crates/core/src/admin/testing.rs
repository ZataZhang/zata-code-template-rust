//! 管理员域端口的内存替身，**只用于测试**。

use std::sync::Mutex;

use async_trait::async_trait;

use super::{Admin, AdminCredentials, AdminId, AdminRepository, AdminRepositoryError};

/// 内存版管理员存储。
#[derive(Default)]
pub struct InMemoryAdminRepository {
    rows: Mutex<Vec<AdminCredentials>>,
}

#[async_trait]
impl AdminRepository for InMemoryAdminRepository {
    async fn create(
        &self,
        username: &str,
        display_name: &str,
        password_hash: &str,
    ) -> Result<Admin, AdminRepositoryError> {
        let mut rows = self.rows.lock().expect("测试替身的锁不应被毒化");
        if rows.iter().any(|row| row.admin.username == username) {
            return Err(AdminRepositoryError::UsernameTaken);
        }
        let admin = Admin {
            id: AdminId(i64::try_from(rows.len()).expect("测试里管理员数不会溢出") + 1),
            username: username.to_string(),
            display_name: display_name.to_string(),
        };
        rows.push(AdminCredentials {
            admin: admin.clone(),
            password_hash: password_hash.to_string(),
        });
        Ok(admin)
    }

    async fn find_credentials_by_username(
        &self,
        username: &str,
    ) -> Result<Option<AdminCredentials>, AdminRepositoryError> {
        let rows = self.rows.lock().expect("测试替身的锁不应被毒化");
        Ok(rows
            .iter()
            .find(|row| row.admin.username == username)
            .cloned())
    }

    async fn find_by_id(&self, admin_id: AdminId) -> Result<Option<Admin>, AdminRepositoryError> {
        let rows = self.rows.lock().expect("测试替身的锁不应被毒化");
        Ok(rows
            .iter()
            .find(|row| row.admin.id == admin_id)
            .map(|row| row.admin.clone()))
    }
}
