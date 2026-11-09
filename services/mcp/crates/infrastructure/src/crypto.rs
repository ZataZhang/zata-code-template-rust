//! AES-256-GCM 凭据认证加密。
//!
//! 密文格式为 `nonce(12 字节) || ciphertext || tag(16 字节)`；附加数据绑定
//! 消费服务、租户与连接 ID，密文被复制到其他连接后无法解密。

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use tpl_mcp_core::{
    CredentialCipher, CredentialCiphertext, CredentialScope, CryptoError, SecretString,
};

/// 随机 nonce 长度。
const NONCE_LEN: usize = 12;

/// 基于 AES-256-GCM 的凭据加密实现。
pub struct AesGcmCredentialCipher {
    cipher: Aes256Gcm,
}

impl AesGcmCredentialCipher {
    /// 使用 32 字节主密钥构造。
    pub fn new(key: &[u8; 32]) -> Self {
        Self {
            cipher: Aes256Gcm::new(key.into()),
        }
    }
}

impl CredentialCipher for AesGcmCredentialCipher {
    fn encrypt(
        &self,
        plaintext: &SecretString,
        scope: &CredentialScope<'_>,
    ) -> Result<CredentialCiphertext, CryptoError> {
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let associated_data = associated_data(scope);
        let encrypted = self
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext.expose().as_bytes(),
                    aad: associated_data.as_bytes(),
                },
            )
            .map_err(|_| CryptoError::Encrypt)?;
        let mut stored = Vec::with_capacity(NONCE_LEN + encrypted.len());
        stored.extend_from_slice(nonce.as_slice());
        stored.extend_from_slice(&encrypted);
        Ok(CredentialCiphertext::new(stored))
    }

    fn decrypt(
        &self,
        ciphertext: &CredentialCiphertext,
        scope: &CredentialScope<'_>,
    ) -> Result<SecretString, CryptoError> {
        let stored = ciphertext.expose().ok_or(CryptoError::Decrypt)?;
        if stored.len() <= NONCE_LEN {
            return Err(CryptoError::Decrypt);
        }
        let (nonce_bytes, encrypted) = stored.split_at(NONCE_LEN);
        let nonce = Nonce::from_slice(nonce_bytes);
        let associated_data = associated_data(scope);
        let plaintext = self
            .cipher
            .decrypt(
                nonce,
                Payload {
                    msg: encrypted,
                    aad: associated_data.as_bytes(),
                },
            )
            .map_err(|_| CryptoError::Decrypt)?;
        String::from_utf8(plaintext)
            .map(SecretString::new)
            .map_err(|_| CryptoError::Decrypt)
    }
}

/// 构造认证加密的附加数据；版本前缀为将来轮换格式留出空间。
fn associated_data(scope: &CredentialScope<'_>) -> String {
    format!(
        "tpl-mcp-credential-v1|{}|{}|{}",
        scope.consumer_id, scope.tenant_id, scope.connection_id
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn scope(connection_id: Uuid) -> CredentialScope<'static> {
        CredentialScope {
            consumer_id: "consumer-a",
            tenant_id: "tenant-a",
            connection_id,
        }
    }

    #[test]
    fn round_trip_and_scope_binding() {
        let cipher = AesGcmCredentialCipher::new(b"0123456789abcdef0123456789abcdef");
        let connection_id = Uuid::from_u128(7);
        let plaintext = SecretString::new("upstream-bearer-token");
        let ciphertext = cipher
            .encrypt(&plaintext, &scope(connection_id))
            .expect("测试加密应当成功");
        let decrypted = cipher
            .decrypt(&ciphertext, &scope(connection_id))
            .expect("同一范围应当可以解密");
        assert_eq!(decrypted.expose(), "upstream-bearer-token");

        let other_connection_id = Uuid::from_u128(8);
        assert_eq!(
            cipher.decrypt(&ciphertext, &scope(other_connection_id)),
            Err(CryptoError::Decrypt)
        );
    }

    #[test]
    fn ciphertext_is_randomised_and_tampering_fails() {
        let cipher = AesGcmCredentialCipher::new(b"0123456789abcdef0123456789abcdef");
        let connection_id = Uuid::from_u128(7);
        let plaintext = SecretString::new("same-token");
        let first = cipher
            .encrypt(&plaintext, &scope(connection_id))
            .expect("第一次加密应当成功");
        let second = cipher
            .encrypt(&plaintext, &scope(connection_id))
            .expect("第二次加密应当成功");
        assert_ne!(first.expose(), second.expose(), "nonce 必须随机");

        let mut tampered = first.expose().expect("应当有密文").to_vec();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x01;
        let tampered = CredentialCiphertext::new(tampered);
        assert_eq!(
            cipher.decrypt(&tampered, &scope(connection_id)),
            Err(CryptoError::Decrypt)
        );
    }

    #[test]
    fn missing_ciphertext_is_rejected() {
        let cipher = AesGcmCredentialCipher::new(b"0123456789abcdef0123456789abcdef");
        assert_eq!(
            cipher.decrypt(&CredentialCiphertext::none(), &scope(Uuid::nil())),
            Err(CryptoError::Decrypt)
        );
    }
}
