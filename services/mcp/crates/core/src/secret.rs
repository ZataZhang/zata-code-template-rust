//! 秘密值包装：凭据明文与密文都不进入日志或 Debug 输出。

use std::fmt;

/// 凭据明文；只在加密、解密与出站请求构造时读取。
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    /// 包装一个明文秘密。
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// 读取明文；调用方不得把它写入日志、错误或响应。
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString(<已隐藏>)")
    }
}

/// 认证加密后的凭据；空值表示该连接没有凭据（`auth_type = none`）。
#[derive(Clone, Default, PartialEq, Eq)]
pub struct CredentialCiphertext(Option<Vec<u8>>);

impl CredentialCiphertext {
    /// 包装密文字节。
    pub fn new(ciphertext: Vec<u8>) -> Self {
        Self(Some(ciphertext))
    }

    /// 无凭据连接。
    pub fn none() -> Self {
        Self(None)
    }

    /// 是否保存了密文。
    pub fn is_configured(&self) -> bool {
        self.0
            .as_ref()
            .is_some_and(|ciphertext| !ciphertext.is_empty())
    }

    /// 读取密文字节；只允许基础设施层持久化或解密。
    pub fn expose(&self) -> Option<&[u8]> {
        self.0.as_deref()
    }
}

impl fmt::Debug for CredentialCiphertext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialCiphertext")
            .field(
                "configured",
                &self.0.as_ref().is_some_and(|bytes| !bytes.is_empty()),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_redacted_in_debug_output() {
        let secret = SecretString::new("super-secret-token");
        assert!(!format!("{secret:?}").contains("super-secret-token"));
        let ciphertext = CredentialCiphertext::new(b"ciphertext".to_vec());
        assert!(!format!("{ciphertext:?}").contains("ciphertext"));
        assert!(format!("{ciphertext:?}").contains("configured: true"));
    }
}
