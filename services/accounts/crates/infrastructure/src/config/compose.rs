//! 解析仅用于 Docker Compose 的受限字面量属性。

use std::collections::BTreeMap;
use std::net::IpAddr;

use super::ComposeEnvironment;

const ALLOWED_KEYS: [&str; 13] = [
    "POSTGRES_PASSWORD",
    "REDIS_PASSWORD",
    "AGENT_TRACING_DB_PASSWORD",
    "AGENT_TRACING_PRODUCERS_JSON",
    "ADMIN_BOOTSTRAP_USERNAME",
    "ADMIN_BOOTSTRAP_PASSWORD",
    "COOKIE_SECURE",
    "TRUSTED_PROXY_CIDR",
    "WEB_PORT",
    "ADMIN_PORT",
    "TRACING_UI_PORT",
    "ACCOUNTS_PORT",
    "LOG_FORMAT",
];

const REQUIRED_KEYS: [&str; 3] = [
    "POSTGRES_PASSWORD",
    "REDIS_PASSWORD",
    "AGENT_TRACING_DB_PASSWORD",
];
const MAX_COMPOSE_PROPERTIES_BYTES: usize = 64 * 1024;

#[derive(Debug, thiserror::Error)]
pub(super) enum ComposePropertiesError {
    /// Compose 属性内容超过大小限制。
    #[error("Nacos Compose 配置超过大小限制")]
    TooLarge,
    /// 配置中的一行不是字面量 `KEY=value`。
    #[error("Nacos Compose 配置第 {line} 行格式无效；只允许字面量 KEY=value")]
    InvalidLine { line: usize },
    /// 配置中包含未允许的键。
    #[error("Nacos Compose 配置第 {line} 行包含未知键")]
    UnknownKey { line: usize },
    /// 配置重复定义同一个键。
    #[error("Nacos Compose 配置第 {line} 行重复定义键 `{key}`")]
    DuplicateKey { line: usize, key: String },
    /// 配置值含有不可由进程环境传递的控制字符。
    #[error("Nacos Compose 配置第 {line} 行包含非法控制字符")]
    InvalidValue { line: usize },
    /// 配置没有提供必需的密码。
    #[error("Nacos Compose 配置缺少非空必填键 `{key}`")]
    MissingRequiredKey { key: &'static str },
    /// 非敏感的部署字段类型或值不合法。
    #[error("Nacos Compose 配置键 `{key}` 的值无效")]
    InvalidTypedValue { key: &'static str },
}

/// 解析 Nacos Compose 属性，只保留允许的字面量键值。
pub(super) fn parse_properties(
    properties: &str,
) -> Result<ComposeEnvironment, ComposePropertiesError> {
    if properties.len() > MAX_COMPOSE_PROPERTIES_BYTES {
        return Err(ComposePropertiesError::TooLarge);
    }

    let mut environment_variables = BTreeMap::new();
    for (line_index, property_line) in properties.lines().enumerate() {
        let line_number = line_index + 1;
        let trimmed_line = property_line.trim_start();
        if trimmed_line.is_empty() || trimmed_line.starts_with('#') || trimmed_line.starts_with('!')
        {
            continue;
        }

        let Some((key, value)) = property_line.split_once('=') else {
            return Err(ComposePropertiesError::InvalidLine { line: line_number });
        };
        if key.is_empty() || key.trim() != key {
            return Err(ComposePropertiesError::InvalidLine { line: line_number });
        }
        if !ALLOWED_KEYS.contains(&key) {
            return Err(ComposePropertiesError::UnknownKey { line: line_number });
        }
        if value.chars().any(char::is_control) {
            return Err(ComposePropertiesError::InvalidValue { line: line_number });
        }
        if environment_variables.contains_key(key) {
            return Err(ComposePropertiesError::DuplicateKey {
                line: line_number,
                key: key.to_string(),
            });
        }
        environment_variables.insert(key.to_string(), value.to_string());
    }

    for key in REQUIRED_KEYS {
        if environment_variables
            .get(key)
            .is_none_or(|value| value.is_empty())
        {
            return Err(ComposePropertiesError::MissingRequiredKey { key });
        }
    }
    validate_typed_values(&environment_variables)?;

    Ok(ComposeEnvironment(environment_variables))
}

fn validate_typed_values(
    environment_variables: &BTreeMap<String, String>,
) -> Result<(), ComposePropertiesError> {
    for key in ["WEB_PORT", "ADMIN_PORT", "TRACING_UI_PORT", "ACCOUNTS_PORT"] {
        if let Some(value) = environment_variables.get(key)
            && value.parse::<u16>().map_or(true, |port| port == 0)
        {
            return Err(ComposePropertiesError::InvalidTypedValue { key });
        }
    }

    if let Some(value) = environment_variables.get("COOKIE_SECURE")
        && !matches!(value.as_str(), "true" | "false")
    {
        return Err(ComposePropertiesError::InvalidTypedValue {
            key: "COOKIE_SECURE",
        });
    }

    if let Some(value) = environment_variables.get("LOG_FORMAT")
        && !matches!(value.as_str(), "text" | "json")
    {
        return Err(ComposePropertiesError::InvalidTypedValue { key: "LOG_FORMAT" });
    }

    if let Some(value) = environment_variables.get("TRUSTED_PROXY_CIDR")
        && !is_valid_ip_or_cidr(value)
    {
        return Err(ComposePropertiesError::InvalidTypedValue {
            key: "TRUSTED_PROXY_CIDR",
        });
    }

    Ok(())
}

fn is_valid_ip_or_cidr(value: &str) -> bool {
    if value.parse::<IpAddr>().is_ok() {
        return true;
    }

    let Some((address, prefix)) = value.split_once('/') else {
        return false;
    };
    let Ok(address) = address.parse::<IpAddr>() else {
        return false;
    };
    let Ok(prefix) = prefix.parse::<u8>() else {
        return false;
    };
    let max_prefix = match address {
        IpAddr::V4(_) => 32,
        IpAddr::V6(_) => 128,
    };
    prefix <= max_prefix
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed_values(properties: &str) -> BTreeMap<String, String> {
        parse_properties(properties)
            .expect("固定的测试属性应当有效")
            .into_variables()
            .collect()
    }

    #[test]
    fn comments_and_empty_lines_are_ignored() {
        let environment_variables = parsed_values(
            "# compose settings\n\n! ignored comment\nPOSTGRES_PASSWORD=pg\nREDIS_PASSWORD=redis\nAGENT_TRACING_DB_PASSWORD=trace\n",
        );
        assert_eq!(environment_variables.len(), 3);
    }

    #[test]
    fn values_are_kept_literally_including_shell_syntax_equals_and_spaces() {
        let environment_variables = parsed_values(
            "POSTGRES_PASSWORD=pg\nREDIS_PASSWORD=redis\nAGENT_TRACING_DB_PASSWORD=trace\nADMIN_BOOTSTRAP_PASSWORD=literal = ${NO_EXPANSION} with spaces\n",
        );
        let actual_password = environment_variables
            .get("ADMIN_BOOTSTRAP_PASSWORD")
            .expect("管理员密码键应保留");
        assert!(
            actual_password == "literal = ${NO_EXPANSION} with spaces",
            "属性解析不能展开、拆分或裁剪值"
        );
    }

    #[test]
    fn duplicate_keys_are_rejected_without_echoing_values() {
        let result = parse_properties(
            "POSTGRES_PASSWORD=first-secret\nPOSTGRES_PASSWORD=second-secret\nREDIS_PASSWORD=redis\n",
        );
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("重复键必须失败"),
        };
        let rendered_error = error.to_string();
        assert!(rendered_error.contains("POSTGRES_PASSWORD"));
        assert!(!rendered_error.contains("first-secret"));
        assert!(!rendered_error.contains("second-secret"));
    }

    #[test]
    fn unknown_keys_are_rejected_without_echoing_values() {
        let result = parse_properties(
            "POSTGRES_PASSWORD=pg\nREDIS_PASSWORD=redis\nDATABASE_URL=postgres://app:secret@localhost/db\n",
        );
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("宿主数据库 URL 不能注入 Compose"),
        };
        let rendered_error = error.to_string();
        assert!(rendered_error.contains("未知键"));
        assert!(!rendered_error.contains("DATABASE_URL"));
        assert!(!rendered_error.contains("secret"));
    }

    #[test]
    fn malformed_lines_and_control_characters_are_rejected() {
        assert!(matches!(
            parse_properties("POSTGRES_PASSWORD=pg\nREDIS_PASSWORD\n"),
            Err(ComposePropertiesError::InvalidLine { line: 2 })
        ));
        assert!(matches!(
            parse_properties("POSTGRES_PASSWORD=pg\nREDIS_PASSWORD=bad\u{0007}value\n"),
            Err(ComposePropertiesError::InvalidValue { line: 2 })
        ));
    }

    #[test]
    fn required_passwords_must_be_present_and_non_empty() {
        assert!(matches!(
            parse_properties("POSTGRES_PASSWORD=pg\n"),
            Err(ComposePropertiesError::MissingRequiredKey {
                key: "REDIS_PASSWORD"
            })
        ));
        assert!(matches!(
            parse_properties("POSTGRES_PASSWORD=\nREDIS_PASSWORD=redis\n"),
            Err(ComposePropertiesError::MissingRequiredKey {
                key: "POSTGRES_PASSWORD"
            })
        ));
    }

    #[test]
    fn non_secret_compose_values_are_validated_before_process_launch() {
        assert!(matches!(
            parse_properties(
                "POSTGRES_PASSWORD=pg\nREDIS_PASSWORD=redis\nAGENT_TRACING_DB_PASSWORD=trace\nWEB_PORT=0\n"
            ),
            Err(ComposePropertiesError::InvalidTypedValue { key: "WEB_PORT" })
        ));
        assert!(matches!(
            parse_properties(
                "POSTGRES_PASSWORD=pg\nREDIS_PASSWORD=redis\nAGENT_TRACING_DB_PASSWORD=trace\nCOOKIE_SECURE=yes\n"
            ),
            Err(ComposePropertiesError::InvalidTypedValue {
                key: "COOKIE_SECURE"
            })
        ));
        assert!(matches!(
            parse_properties(
                "POSTGRES_PASSWORD=pg\nREDIS_PASSWORD=redis\nAGENT_TRACING_DB_PASSWORD=trace\nLOG_FORMAT=debug\n"
            ),
            Err(ComposePropertiesError::InvalidTypedValue { key: "LOG_FORMAT" })
        ));
        assert!(matches!(
            parse_properties(
                "POSTGRES_PASSWORD=pg\nREDIS_PASSWORD=redis\nAGENT_TRACING_DB_PASSWORD=trace\nTRUSTED_PROXY_CIDR=127.0.0.1;\n"
            ),
            Err(ComposePropertiesError::InvalidTypedValue {
                key: "TRUSTED_PROXY_CIDR"
            })
        ));
    }

    #[test]
    fn valid_ports_and_proxy_cidrs_are_accepted() {
        let environment_variables = parsed_values(
            "POSTGRES_PASSWORD=pg\nREDIS_PASSWORD=redis\nAGENT_TRACING_DB_PASSWORD=trace\nWEB_PORT=3000\nADMIN_PORT=8081\nTRACING_UI_PORT=8082\nACCOUNTS_PORT=8083\nCOOKIE_SECURE=false\nLOG_FORMAT=json\nTRUSTED_PROXY_CIDR=172.16.0.0/12\n",
        );
        assert_eq!(
            environment_variables.get("WEB_PORT").map(String::as_str),
            Some("3000")
        );
    }

    #[test]
    fn oversized_content_is_rejected() {
        let oversized_properties = format!(
            "POSTGRES_PASSWORD=pg\nREDIS_PASSWORD=redis\n{}",
            "x".repeat(MAX_COMPOSE_PROPERTIES_BYTES)
        );
        assert!(matches!(
            parse_properties(&oversized_properties),
            Err(ComposePropertiesError::TooLarge)
        ));
    }
}
