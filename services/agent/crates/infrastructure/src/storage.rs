//! 受控文件系统 Blob 存储。
//!
//! 存储键由本服务生成（资源、Skill 资产、Run 产物），不含用户文件名；
//! 写入先落临时文件再原子重命名，读取时拒绝路径穿越与符号链接逃逸。

use std::path::{Component, Path, PathBuf};

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

    /// 校验由服务生成的相对存储键。
    fn validate_storage_key(storage_key: &str) -> Result<(), AgentError> {
        let key_is_valid = !storage_key.is_empty()
            && storage_key.len() <= 512
            && !storage_key.chars().any(char::is_control)
            && !storage_key.contains('\\')
            && storage_key
                .split('/')
                .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
            && !Path::new(storage_key).is_absolute()
            && !Path::new(storage_key).components().any(|component| {
                matches!(
                    component,
                    Component::Prefix(_)
                        | Component::RootDir
                        | Component::CurDir
                        | Component::ParentDir
                )
            });
        if !key_is_valid {
            return Err(AgentError::Internal);
        }
        Ok(())
    }

    /// 逐级创建或解析根目录下的父目录，并拒绝指向根目录外部的符号链接。
    async fn resolve_parent_directory(
        &self,
        storage_key: &str,
        create: bool,
    ) -> Result<Option<(PathBuf, PathBuf, String)>, AgentError> {
        Self::validate_storage_key(storage_key)?;
        let segments = storage_key.split('/').collect::<Vec<_>>();
        let filename = segments.last().ok_or(AgentError::Internal)?.to_string();
        let root_directory = match tokio::fs::canonicalize(&self.root).await {
            Ok(root_directory) => root_directory,
            Err(root_error) if root_error.kind() == std::io::ErrorKind::NotFound && create => {
                tokio::fs::create_dir_all(&self.root)
                    .await
                    .map_err(|error| Self::storage_failure("create_root", &error))?;
                tokio::fs::canonicalize(&self.root)
                    .await
                    .map_err(|error| Self::storage_failure("resolve_root", &error))?
            }
            Err(root_error) if root_error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(root_error) => {
                return Err(Self::storage_failure("resolve_root", &root_error));
            }
        };

        let mut parent_directory = root_directory.clone();
        for directory_segment in segments.iter().take(segments.len().saturating_sub(1)) {
            let directory_path = parent_directory.join(directory_segment);
            if create {
                match tokio::fs::create_dir(&directory_path).await {
                    Ok(()) => {}
                    Err(directory_error)
                        if directory_error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(directory_error) => {
                        return Err(Self::storage_failure("create_dir", &directory_error));
                    }
                }
            }
            let canonical_directory = match tokio::fs::canonicalize(&directory_path).await {
                Ok(canonical_directory) => canonical_directory,
                Err(directory_error) if directory_error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(None);
                }
                Err(directory_error) => {
                    return Err(Self::storage_failure("resolve_dir", &directory_error));
                }
            };
            if !canonical_directory.starts_with(&root_directory) {
                return Err(AgentError::Internal);
            }
            parent_directory = canonical_directory;
        }

        Ok(Some((root_directory, parent_directory, filename)))
    }

    fn storage_failure(operation: &str, error: &std::io::Error) -> AgentError {
        tracing::warn!(operation, error = %error, "Blob 存储操作失败");
        AgentError::StorageUnavailable
    }
}

#[async_trait]
impl BlobStore for FilesystemBlobStore {
    async fn put(&self, storage_key: &str, bytes: &[u8]) -> Result<(), AgentError> {
        let Some((_, parent_directory, filename)) =
            self.resolve_parent_directory(storage_key, true).await?
        else {
            return Err(AgentError::Internal);
        };
        let destination_path = parent_directory.join(filename);
        let temporary_path =
            parent_directory.join(format!(".agent-blob-{}.tmp", uuid::Uuid::new_v4()));
        let write_result = async {
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
        let Some((root_directory, parent_directory, filename)) =
            self.resolve_parent_directory(storage_key, false).await?
        else {
            return Ok(None);
        };
        let blob_path = parent_directory.join(filename);
        let canonical_blob_path = match tokio::fs::canonicalize(&blob_path).await {
            Ok(canonical_blob_path) => canonical_blob_path,
            Err(read_error) if read_error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(read_error) => return Err(Self::storage_failure("resolve_blob", &read_error)),
        };
        if !canonical_blob_path.starts_with(&root_directory) {
            return Err(AgentError::Internal);
        }
        match tokio::fs::read(&canonical_blob_path).await {
            Ok(bytes) => Ok(Some(bytes)),
            Err(read_error) if read_error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(read_error) => Err(Self::storage_failure("read", &read_error)),
        }
    }

    async fn delete(&self, storage_key: &str) -> Result<(), AgentError> {
        let Some((_, parent_directory, filename)) =
            self.resolve_parent_directory(storage_key, false).await?
        else {
            return Ok(());
        };
        let blob_path = parent_directory.join(filename);
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
        for invalid_key in [
            "../escape",
            "/absolute",
            "a//b",
            "a/./b",
            "a/../b",
            r"..\escape",
            r"org\blob",
            "",
        ] {
            assert!(
                blob_store.put(invalid_key, b"x").await.is_err(),
                "键 `{invalid_key}` 必须被拒绝"
            );
        }
        #[cfg(windows)]
        assert!(
            blob_store.put("C:escape", b"x").await.is_err(),
            "Windows 驱动器前缀必须被拒绝"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinks_cannot_escape_the_blob_root() {
        use std::os::unix::fs::symlink;

        let root_directory = tempfile::tempdir().expect("测试根目录应当可创建");
        let outside_directory = tempfile::tempdir().expect("测试外部目录应当可创建");
        std::fs::create_dir(root_directory.path().join("org")).expect("组织目录应当可创建");
        symlink(
            outside_directory.path(),
            root_directory.path().join("org/linked"),
        )
        .expect("目录符号链接应当可创建");
        let blob_store = FilesystemBlobStore::new(root_directory.path());

        assert!(
            blob_store
                .put("org/linked/escape", b"blocked")
                .await
                .is_err()
        );
        assert!(!outside_directory.path().join("escape").exists());

        let outside_blob = outside_directory.path().join("outside.txt");
        std::fs::write(&outside_blob, b"private").expect("外部测试文件应当可写入");
        std::fs::create_dir(root_directory.path().join("org/safe")).expect("安全目录应当可创建");
        symlink(
            &outside_blob,
            root_directory.path().join("org/safe/linked.txt"),
        )
        .expect("文件符号链接应当可创建");

        assert!(blob_store.get("org/safe/linked.txt").await.is_err());
        blob_store.delete("org/safe/linked.txt").await.unwrap();
        assert_eq!(tokio::fs::read(&outside_blob).await.unwrap(), b"private");
    }
}
