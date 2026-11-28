//! The client side of the `/v1` API.

use aegis_api::version_1::{HEALTH_PATH, Health};

use reqwest::StatusCode;

use crate::http::BaseUrl;

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
        let http = BaseUrl::client()?;
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
