//! A2A Agent Card 解析、认证声明与端点校验。

use std::collections::BTreeMap;

use serde::Deserialize;

use super::*;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct LoadedCard {
    pub(super) name: String,
    pub(super) streaming: bool,
    pub(super) endpoint: Url,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct A2aCard {
    name: String,
    #[serde(alias = "supported_interfaces")]
    supported_interfaces: Vec<A2aInterface>,
    capabilities: A2aCapabilities,
    #[serde(alias = "default_input_modes")]
    default_input_modes: Vec<String>,
    #[serde(alias = "default_output_modes")]
    default_output_modes: Vec<String>,
    #[serde(alias = "security_schemes")]
    security_schemes: BTreeMap<String, A2aSecurityScheme>,
    #[serde(alias = "security_requirements")]
    security_requirements: Vec<A2aSecurityRequirement>,
    skills: Vec<A2aSkill>,
}

impl A2aCard {
    pub(super) fn into_loaded(
        self,
        card_url: &Url,
        has_token: bool,
    ) -> Result<LoadedCard, AgentError> {
        if !self
            .default_input_modes
            .iter()
            .any(|mode| mode == TEXT_PLAIN)
            || !self
                .default_output_modes
                .iter()
                .any(|mode| mode == TEXT_PLAIN)
        {
            return Err(request_failed());
        }
        self.validate_security(has_token)?;
        let endpoint = self.select_endpoint(card_url)?;
        let name = if self.name.trim().is_empty() {
            "A2A Agent".to_owned()
        } else {
            self.name.trim().to_owned()
        };
        Ok(LoadedCard {
            name,
            streaming: self.capabilities.streaming,
            endpoint,
        })
    }

    fn validate_security(&self, has_token: bool) -> Result<(), AgentError> {
        let requirements = self
            .security_requirements
            .iter()
            .chain(
                self.skills
                    .iter()
                    .flat_map(|skill| skill.security_requirements.iter()),
            )
            .collect::<Vec<_>>();
        if requirements.is_empty() {
            return Ok(());
        }
        if !has_token {
            return Err(authentication_required());
        }
        for requirement in requirements {
            if requirement.schemes.is_empty() {
                continue;
            }
            if requirement
                .schemes
                .keys()
                .all(|name| self.is_bearer_scheme(name))
            {
                return Ok(());
            }
        }
        Err(request_failed())
    }

    fn is_bearer_scheme(&self, name: &str) -> bool {
        self.security_schemes
            .get(name)
            .and_then(|scheme| scheme.http_auth_security_scheme.as_ref())
            .is_some_and(|scheme| scheme.scheme.eq_ignore_ascii_case("bearer"))
    }

    fn select_endpoint(&self, card_url: &Url) -> Result<Url, AgentError> {
        let card_origin = url_origin(card_url)?;
        for interface in &self.supported_interfaces {
            if interface.protocol_binding != JSONRPC_BINDING {
                continue;
            }
            if !matches!(interface.protocol_version.as_str(), "1.0" | "1.0.0") {
                continue;
            }
            let Ok(endpoint) = Url::parse(&interface.url) else {
                continue;
            };
            let Ok(origin) = url_origin(&endpoint) else {
                continue;
            };
            if origin == card_origin {
                return Ok(endpoint);
            }
        }
        Err(request_failed())
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct A2aInterface {
    url: String,
    #[serde(alias = "protocol_binding")]
    protocol_binding: String,
    #[serde(alias = "protocol_version")]
    protocol_version: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct A2aCapabilities {
    streaming: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct A2aSecurityScheme {
    #[serde(alias = "http_auth_security_scheme")]
    http_auth_security_scheme: Option<A2aHttpAuthScheme>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct A2aHttpAuthScheme {
    scheme: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct A2aSecurityRequirement {
    schemes: BTreeMap<String, A2aStringList>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct A2aStringList {
    list: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct A2aSkill {
    #[serde(alias = "security_requirements")]
    security_requirements: Vec<A2aSecurityRequirement>,
}

pub(super) fn parse_card_url(card_url: &str) -> Result<Url, AgentError> {
    let url = Url::parse(card_url).map_err(|_| request_failed())?;
    url_origin(&url)?;
    Ok(url)
}

fn url_origin(url: &Url) -> Result<(String, String, u16), AgentError> {
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(request_failed());
    }
    let port = url.port_or_known_default().ok_or_else(request_failed)?;
    Ok((
        url.scheme().to_owned(),
        url.host_str().unwrap_or_default().to_ascii_lowercase(),
        port,
    ))
}
