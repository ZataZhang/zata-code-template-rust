//! 密码哈希：argon2id。

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{
    Error as PasswordHashError, PasswordHasher as _, PasswordVerifier as _,
};
use async_trait::async_trait;
use tpl_core::auth::{PasswordHasher, PortFailure};

/// 基于 argon2id 的密码哈希器，使用 `argon2` crate 的默认参数（Argon2id、19 MiB 内存、2 次迭代）。
///
/// 哈希是刻意设计得很耗 CPU 和内存的操作，所以都放到 `spawn_blocking` 线程里执行，
/// 不会卡住异步运行时。哈希结果是带随机盐和参数的 PHC 字符串，将来调整参数后旧哈希仍可校验。
#[derive(Debug, Clone, Copy, Default)]
pub struct Argon2PasswordHasher;

#[async_trait]
impl PasswordHasher for Argon2PasswordHasher {
    async fn hash(&self, password: &str) -> Result<String, PortFailure> {
        let password_bytes = password.as_bytes().to_vec();
        tokio::task::spawn_blocking(move || {
            Argon2::default()
                .hash_password(&password_bytes)
                .map(|password_hash| password_hash.to_string())
        })
        .await
        .map_err(|join_error| PortFailure(format!("哈希任务异常退出：{join_error}")))?
        .map_err(|hash_error| PortFailure(format!("密码哈希失败：{hash_error}")))
    }

    async fn verify(&self, password: &str, stored_hash: &str) -> Result<bool, PortFailure> {
        let password_bytes = password.as_bytes().to_vec();
        let stored_hash = stored_hash.to_owned();
        tokio::task::spawn_blocking(move || {
            let parsed_hash = PasswordHash::new(&stored_hash).map_err(|parse_error| {
                PortFailure(format!("存储的密码哈希格式损坏：{parse_error}"))
            })?;
            match Argon2::default().verify_password(&password_bytes, &parsed_hash) {
                Ok(()) => Ok(true),
                Err(PasswordHashError::PasswordInvalid) => Ok(false),
                Err(other_error) => Err(PortFailure(format!("密码校验失败：{other_error}"))),
            }
        })
        .await
        .map_err(|join_error| PortFailure(format!("校验任务异常退出：{join_error}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hashed_password_verifies_and_wrong_password_does_not() {
        let hasher = Argon2PasswordHasher;
        let stored_hash = hasher.hash("correct-horse").await.unwrap();

        assert!(stored_hash.starts_with("$argon2id$"), "实际：{stored_hash}");
        assert_eq!(hasher.verify("correct-horse", &stored_hash).await, Ok(true));
        assert_eq!(hasher.verify("wrong-horse", &stored_hash).await, Ok(false));
    }

    #[tokio::test]
    async fn the_same_password_gets_a_different_salt_each_time() {
        let hasher = Argon2PasswordHasher;
        let first_hash = hasher.hash("correct-horse").await.unwrap();
        let second_hash = hasher.hash("correct-horse").await.unwrap();

        assert_ne!(first_hash, second_hash);
        assert_eq!(hasher.verify("correct-horse", &second_hash).await, Ok(true));
    }

    #[tokio::test]
    async fn corrupted_stored_hash_is_an_error_not_a_silent_false() {
        let verification = Argon2PasswordHasher
            .verify("anything", "not-a-phc-string")
            .await;
        assert!(
            verification.is_err(),
            "损坏的哈希应报错，实际：{verification:?}"
        );
    }

    #[tokio::test]
    async fn unicode_passwords_round_trip() {
        let hasher = Argon2PasswordHasher;
        let stored_hash = hasher.hash("密码密码密码密码🔑").await.unwrap();
        assert_eq!(
            hasher.verify("密码密码密码密码🔑", &stored_hash).await,
            Ok(true)
        );
    }
}
