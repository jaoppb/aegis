//! The client side of the `/v1` API.

use aegis_api::version_1::{
    ControllerStatus, ENROLLMENTS_PATH, EnrollRequest, EnrollResponse, FLEET_PATH, HEALTH_PATH,
    Health, MAX_REPLY_BYTES, NODE_GONE, RECONCILIATIONS_PATH, ReconcileRequest, SYNCS_PATH,
    StatusReport, SyncRequest, SyncResponse, host_status_path, host_syncs_path, node_path,
};
use std::time::Duration;

use reqwest::{Response, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;

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
    /// The answer is larger than the client reads, so it was dropped unread.
    #[error("the answer is larger than {MAX_REPLY_BYTES} bytes")]
    TooLarge,
    /// The answer is not the JSON that was expected.
    #[error("the answer cannot be read")]
    Undecodable,
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
        let response = self.http.get(self.base.join(FLEET_PATH)).send().await?;
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

    /// Tell an agent to converge its host to `request`'s commit, and read what it did. The answer
    /// is read up to [`MAX_REPLY_BYTES`], and no more is kept.
    ///
    /// # Errors
    ///
    /// The same as [`ApiClient::health`], and [`ClientError::TooLarge`] for an answer beyond the
    /// cap.
    pub async fn reconcile(&self, request: &ReconcileRequest) -> Result<StatusReport, ClientError> {
        let call = self
            .http
            .post(self.base.join(RECONCILIATIONS_PATH))
            .json(request);
        let response = accepted(call.timeout(RECONCILE_TIMEOUT).send().await?)?;
        capped_json(response).await
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
        let url = self.base.join(&host_status_path(report.host()));
        accepted(self.http.put(url).json(report).send().await?).map(drop)
    }

    /// Ask the controller to enrol the node of `request`.
    ///
    /// # Errors
    ///
    /// The same as [`ApiClient::health`].
    pub async fn enroll(&self, request: &EnrollRequest) -> Result<EnrollResponse, ClientError> {
        let response = self.post(ENROLLMENTS_PATH, request).await?;
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

/// The JSON of `response`, read up to [`MAX_REPLY_BYTES`]: a body that announces more, or turns out
/// to hold more while it streams, is abandoned without being buffered whole.
async fn capped_json<Body: DeserializeOwned>(mut response: Response) -> Result<Body, ClientError> {
    let announced = response.content_length().unwrap_or(0);
    let cap = u64::try_from(MAX_REPLY_BYTES).unwrap_or(u64::MAX);
    if announced > cap {
        return Err(ClientError::TooLarge);
    }
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        keep(&mut body, &chunk)?;
    }
    serde_json::from_slice(&body).map_err(|_| ClientError::Undecodable)
}

/// Add `chunk` to `body`, unless that makes it larger than [`MAX_REPLY_BYTES`].
fn keep(body: &mut Vec<u8>, chunk: &[u8]) -> Result<(), ClientError> {
    let size = body.len().saturating_add(chunk.len());
    (size <= MAX_REPLY_BYTES)
        .then_some(())
        .ok_or(ClientError::TooLarge)?;
    body.extend_from_slice(chunk);
    Ok(())
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
