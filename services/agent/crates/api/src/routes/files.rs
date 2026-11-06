//! multipart 上传与二进制下载的共享处理。

use axum::body::Body;
use axum::extract::Multipart;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use tpl_agent_core::error::AgentError;
use tpl_agent_core::resources::{DownloadedBlob, UploadedFile};

use crate::error::ApiError;

/// 读取唯一名为 `file` 的上传字段，并在读取过程中强制文件大小上限。
pub(crate) async fn single_file(
    mut multipart: Multipart,
    max_bytes: usize,
) -> Result<UploadedFile, ApiError> {
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|multipart_error| ApiError::invalid_parameter(multipart_error.body_text()))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let filename = field
            .file_name()
            .map(str::to_owned)
            .ok_or_else(|| ApiError::invalid_parameter("上传字段缺少文件名"))?;
        let media_type = field
            .content_type()
            .map(str::to_owned)
            .unwrap_or_else(|| "application/octet-stream".to_owned());
        let mut bytes = Vec::new();
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|multipart_error| ApiError::invalid_parameter(multipart_error.body_text()))?
        {
            if bytes.len().saturating_add(chunk.len()) > max_bytes {
                return Err(AgentError::PayloadTooLarge.into());
            }
            bytes.extend_from_slice(&chunk);
        }
        return Ok(UploadedFile {
            filename,
            media_type,
            bytes,
        });
    }
    Err(ApiError::invalid_parameter("缺少 file 上传字段"))
}

/// 构造带安全文件名的附件下载响应。
pub(crate) fn download_response(blob: DownloadedBlob) -> Response {
    let ascii_filename = ascii_filename_fallback(&blob.filename);
    let encoded_filename = percent_encode_filename(&blob.filename);
    let content_disposition =
        format!("attachment; filename=\"{ascii_filename}\"; filename*=UTF-8''{encoded_filename}");
    let content_type = HeaderValue::from_str(&blob.media_type)
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
    let disposition = HeaderValue::from_str(&content_disposition)
        .unwrap_or_else(|_| HeaderValue::from_static("attachment"));
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        Body::from(blob.bytes),
    )
        .into_response()
}

fn ascii_filename_fallback(filename: &str) -> String {
    let fallback = filename
        .chars()
        .map(|character| {
            if character.is_ascii_graphic() && !matches!(character, '"' | '\\') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if fallback.is_empty() {
        "download".to_owned()
    } else {
        fallback
    }
}

fn percent_encode_filename(filename: &str) -> String {
    filename
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}
