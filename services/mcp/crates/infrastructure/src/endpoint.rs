//! 出站地址安全策略：拒绝本机、私网、链路本地、组播、保留地址与云元数据目标。
//!
//! 解析结果会随 [`ValidatedEndpoint`] 交给远端客户端固定使用，避免校验后
//! 再次解析造成 DNS rebinding。保存与每次调用前都会重新执行本策略。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use async_trait::async_trait;
use tpl_mcp_core::{EndpointPolicy, EndpointPolicyError, ValidatedEndpoint};
use url::{Host, Url};

const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);

/// 基于 DNS 解析结果的出站地址策略。
pub struct DnsEndpointPolicy {
    allow_loopback: bool,
}

impl DnsEndpointPolicy {
    /// 构造策略；`allow_loopback` 只应为本地开发启用。
    pub fn new(allow_loopback: bool) -> Self {
        Self { allow_loopback }
    }

    fn address_allowed(&self, address: IpAddr) -> bool {
        if address.is_loopback() {
            return self.allow_loopback;
        }
        match address {
            IpAddr::V4(address) => ipv4_is_public(address),
            IpAddr::V6(address) => ipv6_is_public(address),
        }
    }
}

#[async_trait]
impl EndpointPolicy for DnsEndpointPolicy {
    async fn validate(&self, endpoint: &Url) -> Result<ValidatedEndpoint, EndpointPolicyError> {
        if !matches!(endpoint.scheme(), "http" | "https") {
            return Err(EndpointPolicyError::UnsupportedScheme);
        }
        if !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(EndpointPolicyError::InvalidUrl);
        }
        let host = endpoint
            .host_str()
            .filter(|host| !host.is_empty())
            .ok_or(EndpointPolicyError::MissingHost)?;
        let port = endpoint
            .port_or_known_default()
            .ok_or(EndpointPolicyError::MissingHost)?;

        let addresses: Vec<SocketAddr> = match Host::parse(host)
            .map_err(|_| EndpointPolicyError::MissingHost)?
        {
            Host::Ipv4(address) => vec![SocketAddr::new(IpAddr::V4(address), port)],
            Host::Ipv6(address) => vec![SocketAddr::new(IpAddr::V6(address), port)],
            Host::Domain(domain) => {
                if domain.len() > 253 {
                    return Err(EndpointPolicyError::MissingHost);
                }
                let lookup =
                    tokio::time::timeout(RESOLVE_TIMEOUT, tokio::net::lookup_host((domain, port)))
                        .await
                        .map_err(|_| EndpointPolicyError::ResolutionFailed)?
                        .map_err(|_| EndpointPolicyError::ResolutionFailed)?;
                let mut addresses = lookup.collect::<Vec<_>>();
                addresses.sort_unstable();
                addresses.dedup();
                if addresses.is_empty() {
                    return Err(EndpointPolicyError::ResolutionFailed);
                }
                addresses
            }
        };

        let all_loopback = addresses.iter().all(|address| address.ip().is_loopback());
        if endpoint.scheme() == "http" && !(self.allow_loopback && all_loopback) {
            return Err(EndpointPolicyError::InsecureScheme);
        }
        if addresses
            .iter()
            .any(|address| !self.address_allowed(address.ip()))
        {
            return Err(EndpointPolicyError::ForbiddenAddress);
        }

        Ok(ValidatedEndpoint {
            url: endpoint.clone(),
            addresses,
        })
    }
}

/// 只允许全球可路由的 IPv4 地址。
fn ipv4_is_public(address: Ipv4Addr) -> bool {
    if address.is_unspecified()
        || address.is_loopback()
        || address.is_private()
        || address.is_link_local()
        || address.is_broadcast()
        || address.is_multicast()
        || address.is_documentation()
    {
        return false;
    }
    let octets = address.octets();
    // 0.0.0.0/8 "this network"。
    if octets[0] == 0 {
        return false;
    }
    // 100.64.0.0/10 运营商级 NAT。
    if octets[0] == 100 && (64..=127).contains(&octets[1]) {
        return false;
    }
    // 198.18.0.0/15 基准测试。
    if octets[0] == 198 && (18..=19).contains(&octets[1]) {
        return false;
    }
    // 192.0.0.0/24 IETF 协议分配（含云环境常见的特殊地址）。
    if octets[0] == 192 && octets[1] == 0 && octets[2] == 0 {
        return false;
    }
    // 192.88.99.0/24 6to4 中继。
    if octets[0] == 192 && octets[1] == 88 && octets[2] == 99 {
        return false;
    }
    // 240.0.0.0/4 保留。
    if octets[0] >= 240 {
        return false;
    }
    true
}

/// 只允许全球单播 IPv6 地址，并检查内嵌 IPv4 的特殊范围。
fn ipv6_is_public(address: Ipv6Addr) -> bool {
    if let Some(embedded) = address.to_ipv4() {
        return ipv4_is_public(embedded);
    }
    if address.is_unspecified() || address.is_loopback() || address.is_multicast() {
        return false;
    }
    let segments = address.segments();
    // 仅允许 2000::/3 全球单播；fc00::/7、fe80::/10、64:ff9b::/96 等都在此之外。
    if (segments[0] & 0xe000) != 0x2000 {
        return false;
    }
    // 2001:db8::/32 文档地址。
    if segments[0] == 0x2001 && segments[1] == 0x0db8 {
        return false;
    }
    // 2001::/32 Teredo。
    if segments[0] == 0x2001 && segments[1] == 0 {
        return false;
    }
    // 3fff::/20 文档地址。
    if segments[0] == 0x3fff && (segments[1] & 0xf000) == 0 {
        return false;
    }
    // 2002::/16 6to4：内嵌 IPv4 也必须是公网地址。
    if segments[0] == 0x2002 {
        let embedded = Ipv4Addr::new(
            (segments[1] >> 8) as u8,
            segments[1] as u8,
            (segments[2] >> 8) as u8,
            segments[2] as u8,
        );
        return ipv4_is_public(embedded);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(value: &str) -> Url {
        Url::parse(value).expect("测试 URL 有效")
    }

    #[tokio::test]
    async fn public_https_targets_are_accepted() {
        let policy = DnsEndpointPolicy::new(false);
        let validated = policy
            .validate(&url("https://93.184.216.34/mcp"))
            .await
            .expect("公网 HTTPS 地址应当通过");
        assert_eq!(validated.addresses.len(), 1);
        assert_eq!(validated.addresses[0].port(), 443);

        let validated = policy
            .validate(&url("https://8.8.8.8:8443/mcp"))
            .await
            .expect("带端口的公网地址应当通过");
        assert_eq!(validated.addresses[0].port(), 8443);
    }

    #[tokio::test]
    async fn private_loopback_link_local_and_metadata_targets_are_rejected() {
        let policy = DnsEndpointPolicy::new(false);
        for target in [
            "https://10.0.0.1/mcp",
            "https://172.16.4.4/mcp",
            "https://192.168.1.1/mcp",
            "https://127.0.0.1/mcp",
            "https://169.254.169.254/latest/meta-data",
            "https://192.0.2.10/mcp",
            "https://198.18.0.1/mcp",
            "https://[::1]/mcp",
            "https://[fd00::1]/mcp",
            "https://[fe80::1]/mcp",
            "https://[2001:db8::1]/mcp",
            "https://[::ffff:169.254.169.254]/mcp",
        ] {
            let error = policy
                .validate(&url(target))
                .await
                .expect_err("不安全目标必须被拒绝");
            assert_eq!(
                error,
                EndpointPolicyError::ForbiddenAddress,
                "目标 {target} 应命中地址策略"
            );
        }
    }

    #[tokio::test]
    async fn http_is_only_allowed_for_loopback_in_explicit_development_mode() {
        let policy = DnsEndpointPolicy::new(false);
        assert_eq!(
            policy
                .validate(&url("http://127.0.0.1:9000/mcp"))
                .await
                .expect_err("生产策略必须拒绝明文 HTTP"),
            EndpointPolicyError::InsecureScheme
        );

        let development_policy = DnsEndpointPolicy::new(true);
        let validated = development_policy
            .validate(&url("http://127.0.0.1:9000/mcp"))
            .await
            .expect("开发策略允许 loopback HTTP");
        assert_eq!(validated.addresses[0].port(), 9000);
        assert_eq!(
            development_policy
                .validate(&url("http://8.8.8.8:9000/mcp"))
                .await
                .expect_err("开发策略也不允许公网明文 HTTP"),
            EndpointPolicyError::InsecureScheme
        );
        assert_eq!(
            development_policy
                .validate(&url("https://169.254.169.254/latest/meta-data"))
                .await
                .expect_err("开发策略仍拒绝云元数据地址"),
            EndpointPolicyError::ForbiddenAddress
        );
    }

    #[tokio::test]
    async fn malformed_endpoints_are_rejected() {
        let policy = DnsEndpointPolicy::new(false);
        assert_eq!(
            policy
                .validate(&url("ftp://8.8.8.8/mcp"))
                .await
                .expect_err("只支持 HTTP 系协议"),
            EndpointPolicyError::UnsupportedScheme
        );
        assert_eq!(
            policy
                .validate(&url("https://user:secret@8.8.8.8/mcp"))
                .await
                .expect_err("禁止 URL 内嵌凭据"),
            EndpointPolicyError::InvalidUrl
        );
        assert_eq!(
            policy
                .validate(&url("https://8.8.8.8/mcp#fragment"))
                .await
                .expect_err("禁止片段"),
            EndpointPolicyError::InvalidUrl
        );
    }
}
