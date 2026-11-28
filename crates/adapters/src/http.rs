//! What the HTTP clients of the adapters share: the base URL and the client settings.

use std::time::Duration;

/// How long a call may take before it fails.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

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
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .build()
    }

    pub(crate) fn join(&self, path: &str) -> String {
        format!("{}{path}", self.0)
    }

    /// Whether a secret sent to this URL stays protected: it is https, or it is a loopback address
    /// that never leaves the machine.
    pub(crate) fn is_protected(&self) -> bool {
        let loopback = ["http://127.0.0.1", "http://localhost", "http://[::1]"];
        self.0.starts_with("https://") || loopback.iter().any(|host| is_host(&self.0, host))
    }
}

/// Whether `url` is exactly `host`, which may be followed by a port or a path.
fn is_host(url: &str, host: &str) -> bool {
    let tail = url.strip_prefix(host);
    tail.is_some_and(|rest| rest.is_empty() || rest.starts_with([':', '/']))
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
    }
}
