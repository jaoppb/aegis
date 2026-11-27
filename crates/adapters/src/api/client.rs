//! The client side of the `/v1` API.

use aegis_api::version_1::{HEALTH_PATH, Health};
use std::time::Duration;

use reqwest::StatusCode;

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

    fn join(&self, path: &str) -> String {
        format!("{}{path}", self.0)
    }
}

/// A call to the API failed.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The request could not be made or the answer could not be read.
    #[error("the request failed: {0}")]
    Transport(#[from] reqwest::Error),
    /// The server refused the request, for example because the caller lacks the ACL tag.
    #[error("the server answered {0}")]
    Rejected(StatusCode),
}

/// Calls a controller or an agent. Authentication is the caller's tailnet identity, so the client
/// carries no credentials.
#[derive(Debug, Clone)]
pub struct ApiClient {
    http: reqwest::Client,
    base: BaseUrl,
}

impl ApiClient {
    /// A client for the server at `base`.
    ///
    /// # Errors
    ///
    /// Returns a [`ClientError`] when the HTTP client cannot be built.
    pub fn new(base: BaseUrl) -> Result<Self, ClientError> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .build()?;
        Ok(Self { http, base })
    }

    /// Ask the server for its health.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError::Rejected`] when the server refuses the caller, and
    /// [`ClientError::Transport`] when the call or the answer fails.
    pub async fn health(&self) -> Result<Health, ClientError> {
        let response = self.http.get(self.base.join(HEALTH_PATH)).send().await?;
        let status = response.status();
        let accepted = status.is_success();
        accepted
            .then_some(())
            .ok_or(ClientError::Rejected(status))?;
        Ok(response.json().await?)
    }
}
