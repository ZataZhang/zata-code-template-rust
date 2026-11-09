//! 不透明的外部标识：消费服务、租户、用户与分组。
//!
//! 标识只在本服务内作为命名空间字符串保存，不建立到消费项目用户表的跨库外键，
//! 也不假设其内部结构。校验只要求非空、长度受限且不含空白与控制字符。

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// 标识校验失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("外部标识不合法")]
pub struct InvalidOpaqueId;

macro_rules! opaque_id {
    ($name:ident, $label:literal) => {
        #[doc = concat!("不透明外部标识：", $label, "。")]
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            /// 校验并构造标识。
            ///
            /// # Errors
            ///
            /// 空串、超过 128 字符或包含空白/控制字符时返回错误。
            pub fn new(value: impl Into<String>) -> Result<Self, InvalidOpaqueId> {
                let value = value.into();
                if value.is_empty()
                    || value.chars().count() > 128
                    || value
                        .chars()
                        .any(|character| character.is_control() || character.is_whitespace())
                {
                    return Err(InvalidOpaqueId);
                }
                Ok(Self(value))
            }

            /// 以字符串形式读取。
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = InvalidOpaqueId;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::new(value)
            }
        }

        impl TryFrom<String> for $name {
            type Error = InvalidOpaqueId;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

opaque_id!(ConsumerId, "消费服务");
opaque_id!(TenantId, "租户");
opaque_id!(SubjectId, "用户主体");
opaque_id!(GroupId, "分组");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_ids_reject_empty_whitespace_and_long_values() {
        assert!(ConsumerId::new("consumer-a").is_ok());
        assert!(ConsumerId::new("").is_err());
        assert!(TenantId::new("tenant with space").is_err());
        assert!(SubjectId::new("line\nbreak").is_err());
        assert!(GroupId::new("x".repeat(129)).is_err());
    }
}
