//! 对象存储读写客户端。

use std::ops::Range;
use std::time::Duration;

use opendal::layers::TimeoutLayer;
use opendal::services::{Oss, S3};
use opendal::{Operator, Writer};

use crate::config::{ObjectStorageBackend, ObjectStorageConfig};
use crate::error::ObjectStorageError;

// OSS 限制为 1023 字节，S3 为 1024 字节；共享接口采用两者中较小的上限。
const MAX_OBJECT_KEY_BYTES: usize = 1023;
const MAX_BUFFERED_BYTES: usize = 64 * 1024 * 1024;
const MAX_CHUNK_BYTES: usize = 8 * 1024 * 1024;
const MAX_PRESIGN_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// 可共享的对象存储客户端。
#[derive(Clone)]
pub struct ObjectStorageClient {
    operator: Operator,
    max_buffered_bytes: usize,
    max_chunk_bytes: usize,
    max_object_key_bytes: usize,
}

impl ObjectStorageClient {
    /// 根据配置创建对象存储客户端。
    ///
    /// 创建过程只构造客户端，不探测 bucket 或发起网络请求。
    ///
    /// # Errors
    ///
    /// 配置缺少 endpoint、bucket 或凭据，或底层后端配置无效时返回错误。
    pub fn new(config: ObjectStorageConfig) -> Result<Self, ObjectStorageError> {
        validate_config(&config)?;
        let normalized_prefix = normalize_prefix(&config.prefix)?;
        let prefix_separator_bytes = usize::from(!normalized_prefix.is_empty());
        let max_object_key_bytes = MAX_OBJECT_KEY_BYTES
            .checked_sub(normalized_prefix.len() + prefix_separator_bytes)
            .filter(|remaining_bytes| *remaining_bytes > 0)
            .ok_or(ObjectStorageError::InvalidConfiguration(
                "对象前缀过长，没有可用的对象 key 长度",
            ))?;
        let operator = build_operator(&config, &normalized_prefix)?.layer(
            TimeoutLayer::default()
                .with_timeout(Duration::from_secs(config.operation_timeout_secs))
                .with_io_timeout(Duration::from_secs(config.io_timeout_secs)),
        );

        Ok(Self {
            operator,
            max_buffered_bytes: config.max_buffered_bytes,
            max_chunk_bytes: config.max_chunk_bytes,
            max_object_key_bytes,
        })
    }

    /// 将有限大小的对象写入存储。
    ///
    /// 大对象请使用 [`open_writer`](Self::open_writer) 按块写入。
    ///
    /// # Errors
    ///
    /// key 无效、内容超过缓冲上限或后端写入失败时返回错误。
    pub async fn write(
        &self,
        object_key: &str,
        content: Vec<u8>,
    ) -> Result<(), ObjectStorageError> {
        let storage_key = validate_object_key(object_key, self.max_object_key_bytes)?;
        if content.len() > self.max_buffered_bytes {
            return Err(ObjectStorageError::ObjectTooLarge {
                limit_bytes: self.max_buffered_bytes,
            });
        }
        self.operator.write(&storage_key, content).await?;
        Ok(())
    }

    /// 读取不超过配置缓冲上限的对象。
    ///
    /// 对象大于上限时返回 [`ObjectStorageError::ObjectTooLarge`]，不会将完整对象载入内存。
    ///
    /// # Errors
    ///
    /// key 无效、对象超过缓冲上限或后端读取失败时返回错误。
    pub async fn read(&self, object_key: &str) -> Result<Vec<u8>, ObjectStorageError> {
        let storage_key = validate_object_key(object_key, self.max_object_key_bytes)?;
        let reader = self.operator.reader(&storage_key).await?;
        let read_limit = u64::try_from(self.max_buffered_bytes)
            .map_err(|_| ObjectStorageError::InvalidConfiguration("缓冲大小超出支持范围"))?;
        let probe_end = read_limit + 1;
        let content = reader.read(0..probe_end).await?;
        if content.len() > self.max_buffered_bytes {
            return Err(ObjectStorageError::ObjectTooLarge {
                limit_bytes: self.max_buffered_bytes,
            });
        }
        Ok(content.to_vec())
    }

    /// 打开一个按范围分块读取的对象 reader。
    ///
    /// # Errors
    ///
    /// key 无效或后端无法打开对象时返回错误。
    pub async fn open_reader(&self, object_key: &str) -> Result<ObjectReader, ObjectStorageError> {
        let storage_key = validate_object_key(object_key, self.max_object_key_bytes)?;
        let reader = self.operator.reader(&storage_key).await?;
        Ok(ObjectReader {
            reader,
            max_chunk_bytes: self.max_chunk_bytes,
        })
    }

    /// 打开一个按块写入对象的 writer。
    ///
    /// 调用方必须在写入完成后调用 [`ObjectWriter::finish`] 提交对象。
    ///
    /// # Errors
    ///
    /// key 无效或后端无法创建 writer 时返回错误。
    pub async fn open_writer(&self, object_key: &str) -> Result<ObjectWriter, ObjectStorageError> {
        let storage_key = validate_object_key(object_key, self.max_object_key_bytes)?;
        let writer = self.operator.writer(&storage_key).await?;
        Ok(ObjectWriter {
            writer,
            max_chunk_bytes: self.max_chunk_bytes,
        })
    }

    /// 删除一个对象。
    ///
    /// # Errors
    ///
    /// key 无效或后端删除失败时返回错误。
    pub async fn delete(&self, object_key: &str) -> Result<(), ObjectStorageError> {
        let storage_key = validate_object_key(object_key, self.max_object_key_bytes)?;
        self.operator.delete(&storage_key).await?;
        Ok(())
    }

    /// 为对象生成限时下载 URL。
    ///
    /// 返回的 URL 含有访问签名，应视为临时凭据，不得写入日志。
    ///
    /// # Errors
    ///
    /// key 无效、有效期不在 1 秒到 7 天之间，或后端不支持签名读取时返回错误。
    pub async fn presign_read(
        &self,
        object_key: &str,
        expires_in: Duration,
    ) -> Result<String, ObjectStorageError> {
        let storage_key = validate_object_key(object_key, self.max_object_key_bytes)?;
        if expires_in.is_zero() || expires_in > MAX_PRESIGN_TTL {
            return Err(ObjectStorageError::InvalidConfiguration(
                "下载链接有效期必须在 1 秒到 7 天之间",
            ));
        }
        let signed_request = self.operator.presign_read(&storage_key, expires_in).await?;
        Ok(signed_request.uri().to_string())
    }
}

/// 对象的分块 reader。
pub struct ObjectReader {
    reader: opendal::Reader,
    max_chunk_bytes: usize,
}

impl ObjectReader {
    /// 按字节范围读取一个分块。
    ///
    /// `range` 的结束位置不包含在读取结果中。
    ///
    /// # Errors
    ///
    /// 范围无效、超过分块大小上限或后端读取失败时返回错误。
    pub async fn read_chunk(&self, range: Range<u64>) -> Result<Vec<u8>, ObjectStorageError> {
        let Some(range_len) = range.end.checked_sub(range.start) else {
            return Err(ObjectStorageError::InvalidObjectPath(
                "读取范围结束位置早于起始位置",
            ));
        };
        if range_len == 0 {
            return Ok(Vec::new());
        }
        if range_len > self.max_chunk_bytes as u64 {
            return Err(ObjectStorageError::ObjectTooLarge {
                limit_bytes: self.max_chunk_bytes,
            });
        }
        let content = self.reader.read(range).await?;
        Ok(content.to_vec())
    }
}

/// 对象的分块 writer。
pub struct ObjectWriter {
    writer: Writer,
    max_chunk_bytes: usize,
}

impl ObjectWriter {
    /// 向对象追加一个数据块。
    ///
    /// # Errors
    ///
    /// 数据块超过配置上限或后端写入失败时返回错误。
    pub async fn write_chunk(&mut self, content: Vec<u8>) -> Result<(), ObjectStorageError> {
        if content.len() > self.max_chunk_bytes {
            return Err(ObjectStorageError::ObjectTooLarge {
                limit_bytes: self.max_chunk_bytes,
            });
        }
        self.writer.write(content).await?;
        Ok(())
    }

    /// 完成并提交对象。
    ///
    /// # Errors
    ///
    /// 后端提交失败时返回错误。
    pub async fn finish(mut self) -> Result<(), ObjectStorageError> {
        self.writer.close().await?;
        Ok(())
    }
}

fn validate_config(config: &ObjectStorageConfig) -> Result<(), ObjectStorageError> {
    if config.endpoint.trim().is_empty() {
        return Err(ObjectStorageError::InvalidConfiguration(
            "endpoint 不能为空",
        ));
    }
    if config.bucket.trim().is_empty() {
        return Err(ObjectStorageError::InvalidConfiguration("bucket 不能为空"));
    }
    if config.access_key_id.trim().is_empty() || config.access_key_secret().trim().is_empty() {
        return Err(ObjectStorageError::InvalidConfiguration("访问密钥不能为空"));
    }
    if config.operation_timeout_secs == 0 || config.io_timeout_secs == 0 {
        return Err(ObjectStorageError::InvalidConfiguration(
            "超时时间必须大于 0",
        ));
    }
    if config.max_buffered_bytes == 0 || config.max_buffered_bytes > MAX_BUFFERED_BYTES {
        return Err(ObjectStorageError::InvalidConfiguration(
            "缓冲操作大小必须在 1 字节到 64 MiB 之间",
        ));
    }
    if config.max_chunk_bytes == 0 || config.max_chunk_bytes > MAX_CHUNK_BYTES {
        return Err(ObjectStorageError::InvalidConfiguration(
            "分块大小必须在 1 字节到 8 MiB 之间",
        ));
    }
    if config.prefix.len() > MAX_OBJECT_KEY_BYTES
        || config.prefix.chars().any(char::is_control)
        || config.prefix.contains('\\')
        || config
            .prefix
            .trim_matches('/')
            .split('/')
            .any(|segment| matches!(segment, "." | ".."))
    {
        return Err(ObjectStorageError::InvalidConfiguration("对象前缀无效"));
    }
    match config.backend {
        ObjectStorageBackend::Minio => {
            if !config.endpoint.starts_with("http://") && !config.endpoint.starts_with("https://") {
                return Err(ObjectStorageError::InvalidConfiguration(
                    "MinIO endpoint 必须使用 http:// 或 https://",
                ));
            }
            if config.region.trim().is_empty() {
                return Err(ObjectStorageError::InvalidConfiguration(
                    "S3 签名区域不能为空",
                ));
            }
        }
        ObjectStorageBackend::AliyunOss => {
            if !config.endpoint.starts_with("https://") {
                return Err(ObjectStorageError::InvalidConfiguration(
                    "阿里云 OSS endpoint 必须使用 HTTPS",
                ));
            }
        }
    }
    Ok(())
}

fn normalize_prefix(prefix: &str) -> Result<String, ObjectStorageError> {
    let normalized_prefix = prefix.trim_matches('/');
    if normalized_prefix.is_empty() {
        return Ok(String::new());
    }
    validate_object_key(normalized_prefix, MAX_OBJECT_KEY_BYTES)?;
    Ok(normalized_prefix.to_owned())
}

fn validate_object_key(
    object_key: &str,
    max_key_bytes: usize,
) -> Result<String, ObjectStorageError> {
    if object_key.is_empty()
        || object_key.len() > max_key_bytes
        || object_key.starts_with('/')
        || object_key.contains('\\')
        || object_key.chars().any(char::is_control)
        || object_key
            .split('/')
            .any(|segment| matches!(segment, "." | ".."))
    {
        return Err(ObjectStorageError::InvalidObjectPath(
            "key 不能为空、不能以斜线开头、包含控制字符或越过路径前缀",
        ));
    }
    Ok(object_key.to_owned())
}

fn build_operator(
    config: &ObjectStorageConfig,
    normalized_prefix: &str,
) -> Result<Operator, ObjectStorageError> {
    let operator = match config.backend {
        ObjectStorageBackend::Minio => {
            let mut builder = S3::default()
                .endpoint(&config.endpoint)
                .region(&config.region)
                .bucket(&config.bucket)
                .access_key_id(&config.access_key_id)
                .secret_access_key(config.access_key_secret());
            if !normalized_prefix.is_empty() {
                let object_root = format!("/{normalized_prefix}/");
                builder = builder.root(&object_root);
            }
            Operator::new(builder)?.finish()
        }
        ObjectStorageBackend::AliyunOss => {
            let mut builder = Oss::default()
                .endpoint(&config.endpoint)
                .bucket(&config.bucket)
                .access_key_id(&config.access_key_id)
                .access_key_secret(config.access_key_secret());
            if !normalized_prefix.is_empty() {
                let object_root = format!("/{normalized_prefix}/");
                builder = builder.root(&object_root);
            }
            Operator::new(builder)?.finish()
        }
    };
    Ok(operator)
}
