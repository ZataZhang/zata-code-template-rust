//! SHA-256 摘要工具；所有快照与幂等摘要共用同一实现。

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::AgentError;

/// 计算字节内容的十六进制 SHA-256。
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

/// 对可序列化值计算规范 JSON 摘要。
///
/// # Errors
///
/// 序列化失败时返回内部错误，避免把不可验证的快照写入数据库。
pub fn sha256_json<T: Serialize>(value: &T) -> Result<String, AgentError> {
    let serialized = serde_json::to_vec(value).map_err(|_| AgentError::Internal)?;
    Ok(sha256_hex(&serialized))
}
