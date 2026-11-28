//! The client side of the `/v1` API.

use aegis_api::version_1::{
    ControllerStatus, ENROLL_PATH, EnrollRequest, EnrollResponse, HEALTH_PATH, Health, NODE_GONE,
    RECONCILE_PATH, ReconcileRequest, STATUS_PATH, SYNCS_PATH, StatusReport, SyncRequest,
    SyncResponse, host_syncs_path, node_path,
};
use std::time::Duration;

use reqwest::{Response, StatusCode};
use serde::Serialize;

use aegis_domain::{HostName, NodeLabel};

use crate::http::BaseUrl;

/// How long a reconcile may take: pulling images and starting stacks takes far longer than the
/// default request timeout.
const RECONCILE_TIMEOUT: Duration = Duration::from_secs(600);

/// How long a sync may take: the controller waits for every agent, and each may take as long as a
/// reconcile, plus the time to fetch the repository.
const SYNC_TIMEOUT: Duration = Duration::from_mins(11);

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

    /// Ask the server for what it reports.
    ///
    /// # Errors
    ///
    /// The same as [`ApiClient::health`].
    pub async fn controller_status(&self) -> Result<ControllerStatus, ClientError> {
        let response = self.http.get(self.base.join(STATUS_PATH)).send().await?;
        Ok(accepted(response)?.json().await?)
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
        let call = self.http.post(self.base.join(RECONCILE_PATH)).json(request);
        let response = accepted(call.timeout(RECONCILE_TIMEOUT).send().await?)?;
        Ok(response.json().await?)
    }

    /// Ask the controller to sync `host`, or every host when `host` is `None`, now, and wait for
    /// what the agents did.
    ///
    /// # Errors
    ///
    /// The same as [`ApiClient::health`]; the controller answers `409` when no commit has loaded
    /// and `502` when the repository cannot be fetched.
    pub async fn sync(
        &self,
        host: Option<&HostName>,
        request: &SyncRequest,
    ) -> Result<SyncResponse, ClientError> {
        let path = host.map_or_else(
            || SYNCS_PATH.to_owned(),
            |name| host_syncs_path(name.as_str()),
        );
        let call = self.http.post(self.base.join(&path)).json(request);
        let response = accepted(call.timeout(SYNC_TIMEOUT).send().await?)?;
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

    /// Ask the controller to delete the tailnet node `hostname`. A node that does not exist
    /// counts as deleted, but only when the controller says so: any other `404` is a refusal.
    ///
    /// # Errors
    ///
    /// The same as [`ApiClient::health`].
    pub async fn delete_node(&self, hostname: &NodeLabel) -> Result<(), ClientError> {
        let url = self.base.join(&node_path(hostname.as_str()));
        let response = self.http.delete(url).send().await?;
        match response.status() {
            StatusCode::NOT_FOUND => gone(response).await,
            _ => accepted(response).map(drop),
        }
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

/// A `404` answer about a node: success when the controller says the node is not there.
async fn gone(response: Response) -> Result<(), ClientError> {
    let status = response.status();
    let body = response.text().await?;
    (body == NODE_GONE)
        .then_some(())
        .ok_or(ClientError::Rejected(status))
}

fn accepted(response: Response) -> Result<Response, ClientError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    Err(ClientError::Rejected(status))
}
