//! Owner-safe 用户资源、ContextPack 引用与通用产物元数据。

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::AgentError;

/// 允许上传的资源类型；未列出的类型直接拒绝，避免把任意二进制当作上下文。
pub const ALLOWED_RESOURCE_MEDIA_TYPES: &[&str] = &[
    "text/plain",
    "text/markdown",
    "application/json",
    "image/png",
    "image/jpeg",
    "application/pdf",
];

/// 用户上传资源元数据。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserResource {
    /// 资源 UUID。
    pub id: Uuid,
    /// 所属组织。
    pub organization_id: Uuid,
    /// 所有者用户 ID。
    pub owner_user_id: i64,
    /// 用户可见文件名。
    pub filename: String,
    /// MIME 类型。
    pub media_type: String,
    /// 字节数。
    pub size_bytes: i64,
    /// 内容 SHA-256。
    pub sha256: String,
    /// Blob 存储键。
    pub storage_key: String,
    /// 上传时间。
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// 软删除时间。
    #[serde(with = "time::serde::rfc3339::option")]
    pub deleted_at: Option<OffsetDateTime>,
}

/// Run 快照中的资源引用。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceSnapshot {
    /// 资源 UUID。
    pub resource_id: Uuid,
    /// 文件名。
    pub filename: String,
    /// MIME 类型。
    pub media_type: String,
    /// 字节数。
    pub size_bytes: i64,
    /// 内容 SHA-256。
    pub sha256: String,
}

/// 执行时读取到的资源正文。
#[derive(Clone, Debug)]
pub struct ResourceContent {
    /// 元数据快照。
    pub snapshot: ResourceSnapshot,
    /// 已校验摘要的正文。
    pub bytes: Vec<u8>,
}

/// Run 产物元数据。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    /// 产物 UUID。
    pub id: Uuid,
    /// 所属 Run。
    pub run_id: Uuid,
    /// 所有者用户 ID。
    pub owner_user_id: i64,
    /// 文件名。
    pub filename: String,
    /// MIME 类型。
    pub media_type: String,
    /// 字节数。
    pub size_bytes: i64,
    /// 内容 SHA-256。
    pub sha256: String,
    /// Blob 存储键。
    pub storage_key: String,
    /// 创建时间。
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// 执行器返回的待保存产物。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewArtifact {
    /// 文件名。
    pub filename: String,
    /// MIME 类型。
    pub media_type: String,
    /// 正文。
    pub bytes: Vec<u8>,
}

/// 上传文件（用户资源或 Skill 资产）。
#[derive(Clone, Debug)]
pub struct UploadedFile {
    /// 用户可见文件名。
    pub filename: String,
    /// MIME 类型。
    pub media_type: String,
    /// 正文。
    pub bytes: Vec<u8>,
}

/// 下载文件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadedBlob {
    /// 文件名。
    pub filename: String,
    /// MIME 类型。
    pub media_type: String,
    /// 正文。
    pub bytes: Vec<u8>,
}

/// 校验上传元数据与大小。
///
/// # Errors
///
/// 文件名无效、类型不在白名单、空文件或超限时返回错误。
pub fn validate_resource_upload(
    filename: &str,
    media_type: &str,
    size_bytes: usize,
    maximum_bytes: usize,
) -> Result<(), AgentError> {
    if size_bytes == 0 {
        return Err(AgentError::InvalidRequest("资源不能为空文件".to_owned()));
    }
    if size_bytes > maximum_bytes {
        return Err(AgentError::PayloadTooLarge);
    }
    if !ALLOWED_RESOURCE_MEDIA_TYPES.contains(&media_type) {
        return Err(AgentError::UnsupportedMediaType);
    }
    sanitize_filename(filename)?;
    Ok(())
}

/// 规范化下载文件名：去掉路径分隔符与控制字符。
///
/// # Errors
///
/// 文件名在规范化后为空或超长时返回错误。
pub fn sanitize_filename(filename: &str) -> Result<String, AgentError> {
    let basename = filename
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(filename)
        .chars()
        .filter(|character| !character.is_control())
        .take(255)
        .collect::<String>();
    let trimmed = basename.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        return Err(AgentError::InvalidRequest("文件名无效".to_owned()));
    }
    Ok(trimmed.to_owned())
}

/// 校验资源所有者；非所有者按不存在处理。
///
/// # Errors
///
/// 资源不属于当前用户时返回 `resource_not_found`。
pub fn ensure_resource_owned(resource: &UserResource, user_id: i64) -> Result<(), AgentError> {
    if resource.owner_user_id == user_id && resource.deleted_at.is_none() {
        Ok(())
    } else {
        Err(AgentError::NotFound("resource_not_found"))
    }
}

/// 将资源元数据转为 Run 快照引用。
pub fn resource_snapshot(resource: &UserResource) -> ResourceSnapshot {
    ResourceSnapshot {
        resource_id: resource.id,
        filename: resource.filename.clone(),
        media_type: resource.media_type.clone(),
        size_bytes: resource.size_bytes,
        sha256: resource.sha256.clone(),
    }
}

/// 校验执行器产物大小与名称。
///
/// # Errors
///
/// 产物为空、超限或文件名无效时返回错误。
pub fn validate_artifact(artifact: &NewArtifact, maximum_bytes: usize) -> Result<(), AgentError> {
    if artifact.bytes.is_empty() {
        return Err(AgentError::InvalidRequest("产物不能为空".to_owned()));
    }
    if artifact.bytes.len() > maximum_bytes {
        return Err(AgentError::PayloadTooLarge);
    }
    sanitize_filename(&artifact.filename)?;
    if artifact.media_type.len() > 128 || artifact.media_type.chars().any(char::is_control) {
        return Err(AgentError::InvalidRequest("产物 MIME 类型无效".to_owned()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_metadata_is_bounded() {
        assert!(validate_resource_upload("notes.md", "text/markdown", 10, 100).is_ok());
        assert_eq!(
            validate_resource_upload("notes.md", "application/x-sh", 10, 100),
            Err(AgentError::UnsupportedMediaType)
        );
        assert_eq!(
            validate_resource_upload("notes.md", "text/markdown", 101, 100),
            Err(AgentError::PayloadTooLarge)
        );
        assert!(validate_resource_upload("notes.md", "text/markdown", 0, 100).is_err());
    }

    #[test]
    fn filenames_cannot_escape_the_download_header() {
        assert_eq!(sanitize_filename("../../etc/passwd").unwrap(), "passwd");
        assert_eq!(sanitize_filename("C:\\tmp\\a.txt").unwrap(), "a.txt");
        assert!(sanitize_filename("..").is_err());
    }
}
