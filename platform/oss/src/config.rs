//! 对象存储的后端与配置。

use std::fmt;

use serde::Deserialize;

/// 对象存储后端类型。
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObjectStorageBackend {
    /// 本地或自建 MinIO，通过 S3 兼容 API 访问。
    #[default]
    Minio,
    /// 阿里云原生 OSS API。
    AliyunOss,
}

/// 对象存储客户端配置。
///
/// 配置可直接嵌入服务自己的 `Settings` 并从 TOML 与 `APP_OBJECT_STORAGE__*` 环境变量读取。
/// 密钥字段不会通过 `Debug` 输出。
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct ObjectStorageConfig {
    /// 使用的存储后端。
    pub backend: ObjectStorageBackend,
    /// 对象存储 API endpoint。
    pub endpoint: String,
    /// S3 签名区域；MinIO 默认使用 `us-east-1`，阿里云 OSS 原生适配器不使用此字段。
    pub region: String,
    /// 已预先创建的 bucket 名称。
    pub bucket: String,
    /// 对象 key 的统一前缀，用于按服务隔离对象命名空间。
    pub prefix: String,
    /// 访问密钥 ID。
    pub access_key_id: String,
    /// 访问密钥 secret。
    access_key_secret: SecretString,
    /// 控制类操作超时秒数。
    pub operation_timeout_secs: u64,
    /// 读写数据流空闲超时秒数。
    pub io_timeout_secs: u64,
    /// 整体读写缓冲操作的最大字节数。
    pub max_buffered_bytes: usize,
    /// 流式读写单块的最大字节数。
    pub max_chunk_bytes: usize,
}

impl ObjectStorageConfig {
    /// 创建连接到 zata-ops 本机 MinIO 的配置。
    ///
    /// 此构造函数不访问网络；配置内容会在 [`ObjectStorageClient::new`](crate::ObjectStorageClient::new) 时校验。
    pub fn local_minio(
        bucket: impl Into<String>,
        access_key_id: impl Into<String>,
        access_key_secret: impl Into<String>,
    ) -> Self {
        Self {
            bucket: bucket.into(),
            access_key_id: access_key_id.into(),
            access_key_secret: SecretString(access_key_secret.into()),
            ..Self::default()
        }
    }

    /// 创建阿里云 OSS 配置。
    ///
    /// endpoint 应使用 bucket 所属地域的 HTTPS 地址，例如
    /// `https://oss-cn-hangzhou.aliyuncs.com`。
    ///
    /// 此构造函数不访问网络；配置内容会在 [`ObjectStorageClient::new`](crate::ObjectStorageClient::new) 时校验。
    pub fn aliyun_oss(
        endpoint: impl Into<String>,
        bucket: impl Into<String>,
        access_key_id: impl Into<String>,
        access_key_secret: impl Into<String>,
    ) -> Self {
        Self {
            backend: ObjectStorageBackend::AliyunOss,
            endpoint: endpoint.into(),
            bucket: bucket.into(),
            access_key_id: access_key_id.into(),
            access_key_secret: SecretString(access_key_secret.into()),
            ..Self::default()
        }
    }

    /// 设置自定义 endpoint。
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }

    /// 设置 S3 签名区域。
    pub fn with_region(mut self, region: impl Into<String>) -> Self {
        self.region = region.into();
        self
    }

    /// 设置此服务在 bucket 内使用的 key 前缀。
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// 设置控制操作与数据流超时。
    pub fn with_timeouts(mut self, operation_timeout_secs: u64, io_timeout_secs: u64) -> Self {
        self.operation_timeout_secs = operation_timeout_secs;
        self.io_timeout_secs = io_timeout_secs;
        self
    }

    /// 设置整体缓冲操作上限与流式读写块上限。
    pub fn with_size_limits(mut self, max_buffered_bytes: usize, max_chunk_bytes: usize) -> Self {
        self.max_buffered_bytes = max_buffered_bytes;
        self.max_chunk_bytes = max_chunk_bytes;
        self
    }

    pub(crate) fn access_key_secret(&self) -> &str {
        &self.access_key_secret.0
    }
}

impl Default for ObjectStorageConfig {
    fn default() -> Self {
        Self {
            backend: ObjectStorageBackend::Minio,
            endpoint: "http://127.0.0.1:9000".to_owned(),
            region: "us-east-1".to_owned(),
            bucket: String::new(),
            prefix: String::new(),
            access_key_id: String::new(),
            access_key_secret: SecretString::default(),
            operation_timeout_secs: 5,
            io_timeout_secs: 60,
            max_buffered_bytes: 8 * 1024 * 1024,
            max_chunk_bytes: 1024 * 1024,
        }
    }
}

impl fmt::Debug for ObjectStorageConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ObjectStorageConfig")
            .field("backend", &self.backend)
            .field("endpoint", &self.endpoint)
            .field("region", &self.region)
            .field("bucket", &self.bucket)
            .field("prefix", &self.prefix)
            .field("access_key_id", &"<已隐藏>")
            .field("access_key_secret", &"<已隐藏>")
            .field("operation_timeout_secs", &self.operation_timeout_secs)
            .field("io_timeout_secs", &self.io_timeout_secs)
            .field("max_buffered_bytes", &self.max_buffered_bytes)
            .field("max_chunk_bytes", &self.max_chunk_bytes)
            .finish()
    }
}

#[derive(Clone, Default, Deserialize)]
#[serde(transparent)]
struct SecretString(String);

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<已隐藏>")
    }
}
