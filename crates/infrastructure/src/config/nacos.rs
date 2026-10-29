//! 通过 Nacos 3.x Client API 在启动时读取 TOML 配置。

use std::env;
use std::net::IpAddr;
use std::time::Duration;

use figment::Figment;
use figment::providers::{Format, Toml};
use reqwest::{Client, StatusCode, Url};
use serde::Deserialize;

const NACOS_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_NACOS_LOGIN_RESPONSE_BYTES: usize = 16 * 1024;
const MAX_NACOS_CONFIG_RESPONSE_BYTES: usize = 1024 * 1024;

/// Nacos 配置源连接参数。密码与访问令牌不实现 `Debug`，避免意外进入日志。
struct NacosSourceSettings {
    server_addr: Url,
    namespace_id: String,
    group_name: String,
    data_id: String,
    username: Option<String>,
    password: Option<String>,
    access_token: Option<String>,
}

/// 从项目配置文件读取的非敏感 Nacos 配置定位信息。
struct NacosConfigLocator {
    namespace_id: String,
    group_name: String,
    application_data_id: String,
    compose_data_id: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawNacosConfigLocator {
    namespace_id: Option<String>,
    group: Option<String>,
    app_data_id: Option<String>,
    compose_data_id: Option<String>,
}

#[derive(Clone, Copy)]
enum NacosConfigKind {
    Application,
    Compose,
}

/// Nacos Client API 的统一响应。
#[derive(Deserialize)]
struct NacosApiResponse<T> {
    code: i64,
    data: Option<T>,
}

/// Nacos 登录响应。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NacosLoginResponse {
    access_token: String,
}

/// Nacos 配置查询响应中的数据。
#[derive(Deserialize)]
struct NacosConfigData {
    content: String,
}

/// Nacos 配置加载错误。
#[derive(Debug, thiserror::Error)]
pub(super) enum NacosConfigError {
    /// 启用 Nacos 时缺少连接参数。
    #[error("加载 Compose Nacos 配置需要设置 NACOS_SERVER_ADDR")]
    MissingSourceSettings,
    /// 其他 Nacos 参数已设置，但没有服务地址。
    #[error("设置了 Nacos 配置参数，但没有设置 NACOS_SERVER_ADDR")]
    MissingServerAddress,
    /// Nacos 地址不是有效的 HTTP 或 HTTPS URL。
    #[error("NACOS_SERVER_ADDR 必须是有效的 HTTP 或 HTTPS URL")]
    InvalidServerAddress,
    /// 项目配置文件中的 Nacos 定位字段缺失或格式无效。
    #[error(
        "项目配置的 `[nacos]` 区域必须配置 namespace_id、group、app_data_id 和 compose_data_id"
    )]
    InvalidLocator,
    /// 非回环 Nacos 地址必须使用 HTTPS，避免明文传输密钥配置。
    #[error("非本机 Nacos 服务必须使用 HTTPS")]
    InsecureServerAddress,
    /// 环境变量包含非 UTF-8 值。
    #[error("Nacos 环境变量 `{0}` 不是有效的 UTF-8")]
    InvalidEnvironmentValue(&'static str),
    /// 用户名和密码必须同时配置。
    #[error("NACOS_USERNAME 与 NACOS_PASSWORD 必须同时配置")]
    IncompleteCredentials,
    /// 无法创建 HTTP 客户端。
    #[error("无法初始化 Nacos HTTP 客户端")]
    ClientBuild(#[source] reqwest::Error),
    /// Nacos 登录请求失败。
    #[error("Nacos 登录请求失败")]
    LoginTransport(#[source] reqwest::Error),
    /// Nacos 登录返回非成功状态。
    #[error("Nacos 登录被拒绝（HTTP {0}）")]
    LoginStatus(StatusCode),
    /// 读取 Nacos 登录响应时发生网络错误。
    #[error("读取 Nacos 登录响应失败")]
    LoginBodyRead(#[source] reqwest::Error),
    /// 登录响应结构不正确。
    #[error("Nacos 登录响应格式无效")]
    LoginResponse(#[source] serde_json::Error),
    /// Nacos 配置请求失败。
    #[error("Nacos 配置请求失败")]
    ConfigTransport(#[source] reqwest::Error),
    /// Nacos 配置请求返回非成功状态。
    #[error("Nacos 配置读取失败（HTTP {0}）")]
    ConfigStatus(StatusCode),
    /// Nacos API 返回错误码。
    #[error("Nacos 配置 API 返回错误码 {0}")]
    ApiCode(i64),
    /// Nacos 未返回配置内容。
    #[error("Nacos 未返回请求的配置；请检查 namespace、group、dataId 和读取权限")]
    MissingConfig,
    /// Nacos 配置内容为空。
    #[error("Nacos 返回的配置内容为空")]
    EmptyConfig,
    /// 读取 Nacos 配置响应时发生网络错误。
    #[error("读取 Nacos 配置响应失败")]
    ConfigBodyRead(#[source] reqwest::Error),
    /// Nacos 响应结构不正确。
    #[error("Nacos 配置响应格式无效")]
    ConfigResponse(#[source] serde_json::Error),
    /// Nacos 返回的响应超过大小限制。
    #[error("Nacos 响应超过大小限制")]
    ResponseTooLarge,
}

/// 限制 Nacos 响应体大小，避免远端配置意外占用过多内存。
#[derive(Debug, thiserror::Error)]
enum ResponseReadError {
    /// 读取响应体时发生网络错误。
    #[error("读取 Nacos 响应体失败")]
    Transport(#[source] reqwest::Error),
    /// 响应体超过允许大小。
    #[error("Nacos 响应体超过大小限制")]
    TooLarge,
}

/// 从项目配置的 Nacos TOML Data ID 读取配置；未配置服务地址时返回 `None`。
pub(super) async fn load_config_from_environment(
    config_file_path: &str,
) -> Result<Option<String>, NacosConfigError> {
    let Some(source_settings) =
        NacosSourceSettings::from_environment(config_file_path, NacosConfigKind::Application)?
    else {
        return Ok(None);
    };

    request_config_content_from_settings(&source_settings)
        .await
        .map(Some)
}

/// 从项目配置中指定的 Compose Data ID 加载配置；Nacos 服务地址为必填项。
pub(super) async fn load_compose_config_from_environment(
    config_file_path: &str,
) -> Result<String, NacosConfigError> {
    let source_settings =
        NacosSourceSettings::from_environment(config_file_path, NacosConfigKind::Compose)?
            .ok_or(NacosConfigError::MissingSourceSettings)?;

    request_config_content_from_settings(&source_settings).await
}

async fn request_config_content_from_settings(
    source_settings: &NacosSourceSettings,
) -> Result<String, NacosConfigError> {
    let http_client = Client::builder()
        .timeout(NACOS_REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(NacosConfigError::ClientBuild)?;
    let access_token = match source_settings.access_token.as_deref() {
        Some(access_token) => Some(access_token.to_owned()),
        None => match (
            source_settings.username.as_deref(),
            source_settings.password.as_deref(),
        ) {
            (Some(username), Some(password)) => {
                Some(request_access_token(&http_client, source_settings, username, password).await?)
            }
            (None, None) => None,
            _ => return Err(NacosConfigError::IncompleteCredentials),
        },
    };

    request_config_content(&http_client, source_settings, access_token.as_deref()).await
}

impl NacosSourceSettings {
    /// 读取环境凭据和项目配置中的定位信息；设置服务地址即启用远程配置。
    fn from_environment(
        config_file_path: &str,
        config_kind: NacosConfigKind,
    ) -> Result<Option<Self>, NacosConfigError> {
        let server_addr = read_environment_value("NACOS_SERVER_ADDR")?;
        let username = read_environment_value("NACOS_USERNAME")?;
        let password = read_environment_value("NACOS_PASSWORD")?;
        let access_token = read_environment_value("NACOS_ACCESS_TOKEN")?;

        let Some(server_addr) = server_addr else {
            if username.is_some() || password.is_some() || access_token.is_some() {
                return Err(NacosConfigError::MissingServerAddress);
            }
            return Ok(None);
        };

        let locator = NacosConfigLocator::from_config_file(config_file_path)?;
        let data_id = locator.data_id(config_kind).to_string();
        let server_addr = parse_server_address(&server_addr)?;

        Ok(Some(Self {
            server_addr,
            namespace_id: locator.namespace_id,
            group_name: locator.group_name,
            data_id,
            username,
            password,
            access_token,
        }))
    }
}

impl NacosConfigLocator {
    fn from_config_file(config_file_path: &str) -> Result<Self, NacosConfigError> {
        let raw_locator = Figment::new()
            .merge(Toml::file(config_file_path))
            .extract_inner::<RawNacosConfigLocator>("nacos")
            .map_err(|_| NacosConfigError::InvalidLocator)?;
        let namespace_id = required_locator_value(raw_locator.namespace_id)?;
        let group_name = required_locator_value(raw_locator.group)?;
        let application_data_id = required_locator_value(raw_locator.app_data_id)?;
        let compose_data_id = required_locator_value(raw_locator.compose_data_id)?;

        Ok(Self {
            namespace_id,
            group_name,
            application_data_id,
            compose_data_id,
        })
    }

    fn data_id(&self, config_kind: NacosConfigKind) -> &str {
        match config_kind {
            NacosConfigKind::Application => &self.application_data_id,
            NacosConfigKind::Compose => &self.compose_data_id,
        }
    }
}

fn required_locator_value(value: Option<String>) -> Result<String, NacosConfigError> {
    value
        .filter(|value| !value.trim().is_empty() && !value.chars().any(char::is_control))
        .ok_or(NacosConfigError::InvalidLocator)
}

/// 登录 Nacos 获取短时访问令牌，避免将凭据放在 URL 中。
async fn request_access_token(
    http_client: &Client,
    source_settings: &NacosSourceSettings,
    username: &str,
    password: &str,
) -> Result<String, NacosConfigError> {
    let login_url = api_url(&source_settings.server_addr, "auth/user/login");
    let login_response = http_client
        .post(login_url)
        .form(&[("username", username), ("password", password)])
        .send()
        .await
        .map_err(NacosConfigError::LoginTransport)?;
    if !login_response.status().is_success() {
        return Err(NacosConfigError::LoginStatus(login_response.status()));
    }

    let login_body = read_limited_response(login_response, MAX_NACOS_LOGIN_RESPONSE_BYTES)
        .await
        .map_err(|error| match error {
            ResponseReadError::Transport(source) => NacosConfigError::LoginBodyRead(source),
            ResponseReadError::TooLarge => NacosConfigError::ResponseTooLarge,
        })?;
    let login_data = serde_json::from_slice::<NacosLoginResponse>(&login_body)
        .map_err(NacosConfigError::LoginResponse)?;
    if login_data.access_token.is_empty() {
        return Err(NacosConfigError::IncompleteCredentials);
    }
    Ok(login_data.access_token)
}

/// 使用 Nacos Client API 请求一份 TOML 配置。
async fn request_config_content(
    http_client: &Client,
    source_settings: &NacosSourceSettings,
    access_token: Option<&str>,
) -> Result<String, NacosConfigError> {
    let config_url = api_url(&source_settings.server_addr, "client/cs/config");
    let mut config_request = http_client.get(config_url).query(&[
        ("dataId", source_settings.data_id.as_str()),
        ("groupName", source_settings.group_name.as_str()),
        ("namespaceId", source_settings.namespace_id.as_str()),
    ]);
    if let Some(access_token) = access_token {
        config_request = config_request.header("accessToken", access_token);
    }

    let config_response = config_request
        .send()
        .await
        .map_err(NacosConfigError::ConfigTransport)?;
    if !config_response.status().is_success() {
        return Err(NacosConfigError::ConfigStatus(config_response.status()));
    }

    let config_body = read_limited_response(config_response, MAX_NACOS_CONFIG_RESPONSE_BYTES)
        .await
        .map_err(|error| match error {
            ResponseReadError::Transport(source) => NacosConfigError::ConfigBodyRead(source),
            ResponseReadError::TooLarge => NacosConfigError::ResponseTooLarge,
        })?;
    let api_response = serde_json::from_slice::<NacosApiResponse<NacosConfigData>>(&config_body)
        .map_err(NacosConfigError::ConfigResponse)?;
    if api_response.code != 0 {
        return Err(NacosConfigError::ApiCode(api_response.code));
    }

    let config_content = api_response
        .data
        .ok_or(NacosConfigError::MissingConfig)?
        .content;
    if config_content.trim().is_empty() {
        return Err(NacosConfigError::EmptyConfig);
    }
    Ok(config_content)
}

/// 在缓冲响应体前同时检查声明长度和实际分块长度。
async fn read_limited_response(
    mut response: reqwest::Response,
    max_response_bytes: usize,
) -> Result<Vec<u8>, ResponseReadError> {
    if response
        .content_length()
        .is_some_and(|content_length| content_length > max_response_bytes as u64)
    {
        return Err(ResponseReadError::TooLarge);
    }

    let mut response_body = Vec::new();
    while let Some(response_chunk) = response
        .chunk()
        .await
        .map_err(ResponseReadError::Transport)?
    {
        if response_body.len().saturating_add(response_chunk.len()) > max_response_bytes {
            return Err(ResponseReadError::TooLarge);
        }
        response_body.extend_from_slice(&response_chunk);
    }
    Ok(response_body)
}

/// 构造 Nacos API 地址，支持根地址和带 `/nacos` 上下文路径的地址。
fn api_url(server_addr: &Url, endpoint: &str) -> Url {
    let base_path = server_addr.path().trim_end_matches('/');
    let api_path = if base_path.ends_with("/nacos") {
        format!("{base_path}/v3/{endpoint}")
    } else {
        format!("{base_path}/nacos/v3/{endpoint}")
    };
    let mut url = server_addr.clone();
    url.set_path(&api_path);
    url.set_query(None);
    url.set_fragment(None);
    url
}

/// 校验配置的 Nacos 服务地址。
fn parse_server_address(server_address: &str) -> Result<Url, NacosConfigError> {
    let server_url =
        Url::parse(server_address).map_err(|_| NacosConfigError::InvalidServerAddress)?;
    if !matches!(server_url.scheme(), "http" | "https") || server_url.host().is_none() {
        return Err(NacosConfigError::InvalidServerAddress);
    }
    if !server_url.username().is_empty()
        || server_url.password().is_some()
        || server_url.query().is_some()
        || server_url.fragment().is_some()
    {
        return Err(NacosConfigError::InvalidServerAddress);
    }
    if server_url.scheme() == "http" && !is_loopback_address(&server_url) {
        return Err(NacosConfigError::InsecureServerAddress);
    }
    Ok(server_url)
}

/// 判断 Nacos 地址是否只经由本机回环网络访问。
fn is_loopback_address(server_url: &Url) -> bool {
    let Some(host) = server_url.host_str() else {
        return false;
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<IpAddr>()
        .is_ok_and(|address| address.is_loopback())
}

/// 读取一个可选 UTF-8 环境变量，错误信息不包含变量值。
fn read_environment_value(name: &'static str) -> Result<Option<String>, NacosConfigError> {
    match env::var(name) {
        Ok(value) if value.trim().is_empty() => Ok(None),
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err(NacosConfigError::InvalidEnvironmentValue(name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_locator_file(contents: &str) -> (tempfile::TempDir, String) {
        let directory = tempfile::tempdir().expect("测试定位目录应当可创建");
        let path = directory.path().join("project.toml");
        std::fs::write(&path, contents).expect("测试定位文件应当可写入");
        (
            directory,
            path.to_str().expect("临时路径应当是有效 UTF-8").to_owned(),
        )
    }

    #[test]
    fn locator_selects_distinct_application_and_compose_data_ids() {
        let (_directory, config_file_path) = create_locator_file(
            "[nacos]\nnamespace_id = \"test-namespace\"\ngroup = \"TEST\"\napp_data_id = \"app.toml\"\ncompose_data_id = \"compose.properties\"\n",
        );
        let locator = NacosConfigLocator::from_config_file(&config_file_path)
            .expect("固定测试定位信息应当有效");
        assert_eq!(locator.namespace_id, "test-namespace");
        assert_eq!(locator.group_name, "TEST");
        assert_eq!(locator.data_id(NacosConfigKind::Application), "app.toml");
        assert_eq!(
            locator.data_id(NacosConfigKind::Compose),
            "compose.properties"
        );
    }

    #[test]
    fn locator_validation_errors_do_not_echo_invalid_values() {
        let (_directory, config_file_path) = create_locator_file(
            "[nacos]\nnamespace_id = \"secret pasted here\"\ngroup = \"TEST\"\napp_data_id = \"app.toml\"\n",
        );
        let error = match NacosConfigLocator::from_config_file(&config_file_path) {
            Err(error) => error,
            Ok(_) => panic!("缺少 Compose Data ID 的定位信息必须失败"),
        };
        assert!(!error.to_string().contains("secret pasted here"));
    }
}
