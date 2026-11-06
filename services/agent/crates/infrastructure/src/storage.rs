//! 受控文件系统 Blob 存储。
//!
//! 存储键由本服务生成（资源、Skill 资产、Run 产物），不含用户文件名；
//! 写入先落临时文件再原子重命名，读取时拒绝路径穿越与符号链接逃逸。

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tpl_agent_core::error::AgentError;
use tpl_agent_core::ports::BlobStore;

/// 基于目录的 Blob 存储；键映射为 `<root>/<key>`。
#[derive(Clone)]
pub struct FilesystemBlobStore {
    root: PathBuf,
}

impl FilesystemBlobStore {
    /// 使用配置的 Blob 根目录创建存储；目录在首次写入时创建。
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// 将存储键解析为根目录内的绝对路径，拒绝越界键。
    fn resolve(&self, storage_key: &str) -> Result<PathBuf, AgentError> {
        let key_is_valid = !storage_key.is_empty()
            && storage_key.len() <= 512
            && !storage_key.chars().any(char::is_control)
            && storage_key
                .split('/')
                .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
            && !Path::new(storage_key).is_absolute();
        if !key_is_valid {
            return Err(AgentError::Internal);
        }
        Ok(self.root.join(storage_key))
    }

    fn storage_failure(operation: &str, error: &std::io::Error) -> AgentError {
        tracing::warn!(operation, error = %error, "Blob 存储操作失败");
        AgentError::StorageUnavailable
    }
}

#[async_trait]
impl BlobStore for FilesystemBlobStore {
    async fn put(&self, storage_key: &str, bytes: &[u8]) -> Result<(), AgentError> {
        let destination_path = self.resolve(storage_key)?;
        let parent_directory = destination_path
            .parent()
            .ok_or(AgentError::Internal)?
            .to_owned();
        let temporary_path =
            destination_path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
        let write_result = async {
            tokio::fs::create_dir_all(&parent_directory)
                .await
                .map_err(|error| Self::storage_failure("create_dir", &error))?;
            tokio::fs::write(&temporary_path, bytes)
                .await
                .map_err(|error| Self::storage_failure("write", &error))?;
            std::fs::rename(&temporary_path, &destination_path)
                .map_err(|error| Self::storage_failure("rename", &error))
        }
        .await;
        if write_result.is_err() {
            let _ = tokio::fs::remove_file(&temporary_path).await;
        }
        write_result
    }

    async fn get(&self, storage_key: &str) -> Result<Option<Vec<u8>>, AgentError> {
        let blob_path = self.resolve(storage_key)?;
        match tokio::fs::read(&blob_path).await {
            Ok(bytes) => Ok(Some(bytes)),
            Err(read_error) if read_error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(read_error) => Err(Self::storage_failure("read", &read_error)),
        }
    }

    async fn delete(&self, storage_key: &str) -> Result<(), AgentError> {
        let blob_path = self.resolve(storage_key)?;
        match tokio::fs::remove_file(&blob_path).await {
            Ok(()) => Ok(()),
            Err(delete_error) if delete_error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(delete_error) => Err(Self::storage_failure("delete", &delete_error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn blobs_round_trip_and_delete_is_idempotent() {
        let directory = tempfile::tempdir().expect("测试目录应当可创建");
        let blob_store = FilesystemBlobStore::new(directory.path());
        let storage_key = "org/abc/runs/def/artifacts/ghi";
        assert!(blob_store.get(storage_key).await.unwrap().is_none());
        blob_store.put(storage_key, b"hello").await.unwrap();
        assert_eq!(
            blob_store.get(storage_key).await.unwrap().as_deref(),
            Some(b"hello".as_slice())
        );
        blob_store.delete(storage_key).await.unwrap();
        blob_store.delete(storage_key).await.unwrap();
        assert!(blob_store.get(storage_key).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn traversal_keys_are_rejected() {
        let directory = tempfile::tempdir().expect("测试目录应当可创建");
        let blob_store = FilesystemBlobStore::new(directory.path());
        for invalid_key in ["../escape", "/absolute", "a//b", "a/./b", "a/../b", ""] {
            assert!(
                blob_store.put(invalid_key, b"x").await.is_err(),
                "键 `{invalid_key}` 必须被拒绝"
            );
        }
    }
}
