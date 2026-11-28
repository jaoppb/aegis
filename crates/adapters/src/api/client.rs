//! The client side of the `/v1` API.

use aegis_api::version_1::{
    ENROLL_PATH, EnrollRequest, EnrollResponse, HEALTH_PATH, Health, RECONCILE_PATH,
    ReconcileRequest, STATUS_PATH, StatusReport,
};
use reqwest::{Response, StatusCode};
use serde::Serialize;

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
        Ok(accepted(response)?.json().await?)
    }

    /// Tell an agent to converge its host to `request`'s commit, and read what it did.
    ///
    /// # Errors
    ///
    /// The same as [`ApiClient::health`].
    pub async fn reconcile(&self, request: &ReconcileRequest) -> Result<StatusReport, ClientError> {
        let response = self.post(RECONCILE_PATH, request).await?;
        Ok(response.json().await?)
    }

    /// Send the controller what an agent did.
    ///
    /// # Errors
    ///
    /// The same as [`ApiClient::health`].
    pub async fn report_status(&self, report: &StatusReport) -> Result<(), ClientError> {
        self.post(STATUS_PATH, report).await.map(drop)
    }

    /// Ask the controller to enrol the node of `request`.
    ///
    /// # Errors
    ///
    /// The same as [`ApiClient::health`].
    pub async fn enroll(&self, request: &EnrollRequest) -> Result<EnrollResponse, ClientError> {
        let response = self.post(ENROLL_PATH, request).await?;
        Ok(response.json().await?)
    }

    async fn post<Body: Serialize + Sync>(
        &self,
        path: &str,
        body: &Body,
    ) -> Result<Response, ClientError> {
        let call = self.http.post(self.base.join(path)).json(body);
        accepted(call.send().await?)
    }
}

fn accepted(response: Response) -> Result<Response, ClientError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    Err(ClientError::Rejected(status))
}
