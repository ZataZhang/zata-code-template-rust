//! 共享对象存储平台 crate。
//!
//! 为业务服务提供一致的对象读写接口，底层支持本地 MinIO 与阿里云 OSS。
//! 本 crate 不承载具体业务规则；服务应在自己的 infrastructure 层使用它，并由 composition root 装配。

mod client;
mod config;
mod error;

pub use client::{ObjectReader, ObjectStorageClient, ObjectWriter};
pub use config::{ObjectStorageBackend, ObjectStorageConfig};
pub use error::ObjectStorageError;
