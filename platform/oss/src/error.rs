//! 对象存储统一错误类型。

/// 对象存储配置或操作失败。
#[derive(Debug, thiserror::Error)]
pub enum ObjectStorageError {
    /// 对象存储配置无效。
    #[error("对象存储配置无效：{0}")]
    InvalidConfiguration(&'static str),
    /// 对象 key 或读取范围无效。
    #[error("对象路径或读取范围无效：{0}")]
    InvalidObjectPath(&'static str),
    /// 缓冲读写对象超过配置的大小上限。
    #[error("对象超过缓冲操作大小上限 {limit_bytes} 字节")]
    ObjectTooLarge {
        /// 允许缓冲的最大字节数。
        limit_bytes: usize,
    },
    /// 底层对象存储 SDK 操作失败。
    #[error("对象存储操作失败")]
    Backend(#[source] opendal::Error),
}

impl From<opendal::Error> for ObjectStorageError {
    fn from(error: opendal::Error) -> Self {
        Self::Backend(error)
    }
}
