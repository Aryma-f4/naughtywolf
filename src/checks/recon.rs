//! Bounded observation of one approved asset; no port sweep or redirect traversal.
use super::{CheckFinding, CheckResult};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    net::{IpAddr, SocketAddr},
    time::Duration,
};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceReconInput {
    #[serde(default)]
    pub probe_http: bool,
}

pub fn target_url(address: &str) -> Result<Url, &'static str> {
    let value = address.trim();
    if value.is_empty() || value.len() > 2048 || address.chars().any(char::is_control) {
        return Err("Asset address is empty or invalid.");
    }
    let value = if value.contains("://") {
        value.to_owned()
    } else if value.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("https://[{value}]/")
    } else {
        format!("https://{value}")
    };
    let mut url = Url::parse(&value).map_err(|_| "Use a hostname, IP address, or HTTP(S) URL.")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port_or_known_default() == Some(0)
    {
        return Err("Use an HTTP(S) target without embedded credentials.");
    }
    // Observe the asset's origin; do not transmit saved paths, tokens or fragments.
    url.set_path("/");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn permitted_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            !ip.is_unspecified() && !ip.is_multicast() && !ip.is_broadcast() && !ip.is_link_local()
        }
        IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map(|ip| permitted_address(IpAddr::V4(ip)))
            .unwrap_or_else(|| {
                !ip.is_unspecified() && !ip.is_multicast() && (ip.segments()[0] & 0xffc0) != 0xfe80
            }),
    }
}

fn finding(code: &str, message: impl Into<String>) -> CheckFinding {
    CheckFinding {
        code: code.to_owned(),
        message: message.into().chars().take(512).collect(),
    }
}

pub async fn collect(address: &str, input: SurfaceReconInput) -> Result<CheckResult, &'static str> {
    let url = target_url(address)?;
    let host = url
        .host_str()
        .ok_or("Target has no hostname.")?
        .trim_matches(['[', ']']);
    let port = url.port_or_known_default().ok_or("Target has no port.")?;
    let resolved = tokio::time::timeout(
        Duration::from_secs(4),
        tokio::net::lookup_host((host, port)),
    )
    .await
    .map_err(|_| "DNS lookup timed out.")?
    .map_err(|_| "DNS lookup failed.")?;
    let addresses: BTreeSet<IpAddr> = resolved.map(|address| address.ip()).collect();
    if addresses.is_empty() {
        return Err("No DNS addresses were returned.");
    }
    if addresses.iter().any(|ip| !permitted_address(*ip)) {
        return Err("Unspecified, multicast, and link-local destinations are not supported.");
    }
    let mut findings = vec![finding("recon.target", url.as_str())];
    for ip in addresses.iter().take(16) {
        findings.push(finding("dns.address", ip.to_string()));
    }
    if addresses.len() > 16 {
        findings.push(finding("dns.limit", "First 16 resolved addresses shown."));
    }
    if !input.probe_http {
        return Ok(CheckResult::succeeded(findings));
    }
    let sockets: Vec<SocketAddr> = addresses
        .iter()
        .take(16)
        .map(|ip| SocketAddr::new(*ip, port))
        .collect();
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(8))
        .user_agent("NaughtyWolf-AssetRecon/1.0")
        .resolve_to_addrs(host, &sockets)
        .build()
        .map_err(|_| "Could not initialize the HTTP probe.")?;
    let response = match client.head(url.clone()).send().await {
        Ok(response) => response,
        Err(_) => {
            let mut result = CheckResult::succeeded(findings);
            result.state = super::RunState::Failed;
            result.error =
                Some("DNS completed; HTTP connection or TLS validation failed.".to_owned());
            return Ok(result);
        }
    };
    findings.push(finding(
        "http.status",
        response.status().as_u16().to_string(),
    ));
    for header in [
        "server",
        "content-type",
        "strict-transport-security",
        "content-security-policy",
        "x-frame-options",
        "x-content-type-options",
    ] {
        if let Some(value) = response.headers().get(header).and_then(|h| h.to_str().ok()) {
            findings.push(finding(&format!("http.{header}"), value));
        }
    }
    if response.status().is_redirection() {
        findings.push(finding(
            "http.redirect",
            "Redirect observed; destination was not followed.",
        ));
    }
    findings.push(finding(
        "tls.validation",
        if url.scheme() == "https" {
            "HTTPS certificate and hostname validated by the HTTP client."
        } else {
            "Not checked: the selected origin uses HTTP."
        },
    ));
    Ok(CheckResult::succeeded(findings))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes_origins_and_discards_tokens() {
        assert_eq!(
            target_url("example.test/path?token=secret#part")
                .unwrap()
                .as_str(),
            "https://example.test/"
        );
        assert_eq!(
            target_url("http://127.0.0.1:8080/a").unwrap().as_str(),
            "http://127.0.0.1:8080/"
        );
        assert_eq!(target_url("::1").unwrap().as_str(), "https://[::1]/");
    }
    #[test]
    fn rejects_credentials_schemes_and_control_characters() {
        for value in [
            "",
            "file:///etc/passwd",
            "https://user:secret@example.test",
            "example.test\n",
            "http://example.test:0",
        ] {
            assert!(target_url(value).is_err(), "{value}");
        }
    }
    #[test]
    fn excludes_metadata_and_non_host_addresses_but_allows_lab_networks() {
        for value in [
            "169.254.169.254",
            "0.0.0.0",
            "224.0.0.1",
            "fe80::1",
            "::ffff:169.254.1.2",
        ] {
            assert!(!permitted_address(value.parse().unwrap()));
        }
        for value in ["192.0.2.10", "10.0.0.5", "127.0.0.1", "::1"] {
            assert!(permitted_address(value.parse().unwrap()));
        }
    }
    #[tokio::test]
    async fn dns_only_result_has_no_http_claims() {
        let result = collect("192.0.2.10", SurfaceReconInput::default())
            .await
            .unwrap();
        assert!(
            result
                .findings
                .iter()
                .any(|f| f.code == "dns.address" && f.message == "192.0.2.10")
        );
        assert!(!result.findings.iter().any(|f| f.code.starts_with("http.")));
    }

    #[tokio::test]
    async fn http_probe_uses_head_discards_sensitive_headers_and_does_not_follow_redirects() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = vec![0; 4096];
            let count = stream.read(&mut bytes).await.unwrap();
            let request = String::from_utf8_lossy(&bytes[..count]).to_string();
            stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://192.0.2.99/private\r\nServer: lab-fixture\r\nSet-Cookie: secret=do-not-store\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
            request
        });
        let result = collect(
            &format!("http://{addr}/private?token=do-not-send"),
            SurfaceReconInput { probe_http: true },
        )
        .await
        .unwrap();
        let request = server.await.unwrap();
        assert!(request.starts_with("HEAD / HTTP/1.1\r\n"));
        assert!(!request.contains("do-not-send"));
        assert_eq!(result.state, super::super::RunState::Succeeded);
        assert!(
            result
                .findings
                .iter()
                .any(|f| f.code == "http.status" && f.message == "302")
        );
        assert!(
            result
                .findings
                .iter()
                .any(|f| f.code == "http.server" && f.message == "lab-fixture")
        );
        let serialized = serde_json::to_string(&result).unwrap();
        assert!(!serialized.contains("do-not-store"));
        assert!(!serialized.contains("192.0.2.99"));
    }
}
