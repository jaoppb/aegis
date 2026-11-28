//! What the HTTP clients of the adapters share: the base URL and the client settings.

use std::net::IpAddr;
use std::time::Duration;

/// How long a call may take before it fails.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Make the pure-Rust `ring` provider the process default for TLS, which `reqwest` needs and
/// does not choose itself. A provider that is already installed stays in place.
fn install_crypto_provider() {
    drop(rustls::crypto::ring::default_provider().install_default());
}

/// The root URL of a server, such as `http://100.64.0.7:7480`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseUrl(String);

impl BaseUrl {
    /// The server at `url`; a trailing slash is dropped.
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        let url: String = url.into();
        Self(url.trim_end_matches('/').to_owned())
    }

    /// A client for the servers behind a base URL: it never follows a redirect, so a credential is
    /// never forwarded to another host, and it gives up after the request timeout.
    pub(crate) fn client() -> Result<reqwest::Client, reqwest::Error> {
        install_crypto_provider();
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .build()
    }

    pub(crate) fn join(&self, path: &str) -> String {
        format!("{}{path}", self.0)
    }

    /// Whether a secret sent to this URL stays protected: it is `https`, or `http` to a loopback
    /// host that never leaves the machine, and it carries no user info that could hide another host.
    pub(crate) fn is_protected(&self) -> bool {
        reqwest::Url::parse(&self.0).is_ok_and(|url| is_protected_url(&url))
    }
}

fn is_protected_url(url: &reqwest::Url) -> bool {
    if !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    match url.scheme() {
        "https" => true,
        "http" => is_loopback(url),
        _ => false,
    }
}

fn is_loopback(url: &reqwest::Url) -> bool {
    let host = url.host_str().unwrap_or_default();
    let address = host.trim_matches(['[', ']']).parse::<IpAddr>();
    host == "localhost" || address.is_ok_and(|found| found.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::BaseUrl;

    fn protected(url: &str) -> bool {
        BaseUrl::new(url).is_protected()
    }

    #[test]
    fn https_and_loopback_are_protected() {
        assert!(protected("https://api.tailscale.com"));
        assert!(protected("http://127.0.0.1:8080"));
        assert!(protected("http://localhost"));
        assert!(protected("http://[::1]:9"));
    }

    #[test]
    fn other_plain_http_is_not() {
        assert!(!protected("http://api.tailscale.com"));
        assert!(!protected("http://127.0.0.1.evil.com"));
        assert!(!protected("http://localhost.evil.com:80"));
        assert!(!protected("ftp://127.0.0.1"));
        assert!(!protected("not a url"));
    }

    #[test]
    fn user_info_cannot_hide_another_host() {
        assert!(!protected("http://127.0.0.1:x@evil.com/"));
        assert!(!protected("http://localhost:80@evil.com"));
        assert!(!protected("https://user:pass@api.tailscale.com"));
    }
}
