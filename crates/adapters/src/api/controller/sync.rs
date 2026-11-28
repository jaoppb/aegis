//! `POST /v1/syncs` and `POST /v1/hosts/{host}/syncs`: an operator asks the controller to look at
//! the repository now and make the agents converge, instead of waiting for the next poll.

use std::sync::Arc;

use aegis_api::version_1::{
    HOST_SYNCS_PATH, HostSync, ReconcileRequest, SYNCS_PATH, SyncRequest, SyncResponse,
};
use aegis_application::{Notice, Poller, Upstream};
use aegis_domain::{CommitSha, HostName, StackName};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use tokio::task::JoinHandle;

use super::{Asked, Shared, Verified, ask, look};

/// What a sync handler works with: the controller's state and the poller that fetches.
struct Syncing<Up> {
    shared: Arc<Shared>,
    poller: Arc<Poller<Up>>,
}

impl<Up> Clone for Syncing<Up> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
            poller: Arc::clone(&self.poller),
        }
    }
}

/// The routes of the sync resource, to be served behind the operator check.
pub(super) fn routes<Up>(shared: Arc<Shared>, poller: Arc<Poller<Up>>) -> Router
where
    Up: Upstream + Send + Sync + 'static,
{
    Router::new()
        .route(SYNCS_PATH, post(sync_all::<Up>))
        .route(HOST_SYNCS_PATH, post(sync_host::<Up>))
        .with_state(Syncing { shared, poller })
}

/// A refused sync: a status and a short plain-text reason that holds no secrets.
struct Refusal {
    status: StatusCode,
    reason: String,
}

impl Refusal {
    fn new(status: StatusCode, reason: &impl std::fmt::Display) -> Self {
        Self {
            status,
            reason: reason.to_string(),
        }
    }
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        (self.status, self.reason).into_response()
    }
}

/// The stack a request limits the sync to, when it names one.
fn stack_of(request: &SyncRequest) -> Result<Option<StackName>, Refusal> {
    let named = request.stack().map(StackName::new);
    named
        .transpose()
        .map_err(|error| Refusal::new(StatusCode::BAD_REQUEST, &error))
}

/// Look at the repository now and give the commit the agents should converge to. A remote that
/// cannot be fetched, or a commit that cannot be read, is no commit to sync to.
async fn refresh<Up>(syncing: &Syncing<Up>) -> Result<CommitSha, Refusal>
where
    Up: Upstream + Send + Sync + 'static,
{
    look(&syncing.shared, &syncing.poller).await;
    let book = syncing.shared.book();
    match book.fleet.notice() {
        Notice::Unreachable(reason) => Err(Refusal::new(StatusCode::BAD_GATEWAY, reason)),
        Notice::Unloadable(found) => Err(Refusal::new(
            StatusCode::CONFLICT,
            &format!("commit {} cannot be read", found.commit()),
        )),
        Notice::Clear => book
            .fleet
            .goal()
            .map(|goal| goal.commit().clone())
            .ok_or_else(|| Refusal::new(StatusCode::CONFLICT, &"no commit has loaded yet")),
    }
}

/// Ask the agent of `host` to converge, and say how it went. A whole-host sync is recorded like a
/// reconcile of the poll; a sync of one stack only frees the host, because its report does not
/// stand for the host.
async fn converge(shared: Arc<Shared>, host: HostName, request: ReconcileRequest) -> HostSync {
    match ask(&shared, &host, &request).await {
        Asked::Skipped(reason) => HostSync::failed(host.as_str(), reason),
        Asked::Answered(outcome) => {
            let sync = summary(&host, &outcome);
            let recorded = request.stack().is_none().then_some(outcome);
            shared.book().conclude(&host, recorded);
            sync
        }
    }
}

fn summary(host: &HostName, outcome: &Result<Verified, String>) -> HostSync {
    match outcome {
        Ok(verified) => HostSync::done(host.as_str(), verified.report.clone()),
        Err(reason) => HostSync::failed(host.as_str(), reason.clone()),
    }
}

/// A host's sync that was started and has not answered yet.
struct Pending {
    host: HostName,
    task: JoinHandle<HostSync>,
}

/// Sync `hosts` at the same time and collect the answers in the order of `hosts`.
async fn converge_all(
    shared: &Arc<Shared>,
    hosts: Vec<HostName>,
    request: &ReconcileRequest,
) -> SyncResponse {
    let start = |host: HostName| {
        let call = converge(Arc::clone(shared), host.clone(), request.clone());
        Pending {
            host,
            task: tokio::spawn(call),
        }
    };
    let pending: Vec<Pending> = hosts.into_iter().map(start).collect();
    let mut done = Vec::new();
    for each in pending {
        let name = each.host;
        let failed = || HostSync::failed(name.as_str(), "the sync task failed");
        done.push(each.task.await.unwrap_or_else(|_| failed()));
    }
    SyncResponse::new(done)
}

async fn run<Up>(
    syncing: &Syncing<Up>,
    hosts: Vec<HostName>,
    request: &SyncRequest,
) -> Result<Json<SyncResponse>, Refusal>
where
    Up: Upstream + Send + Sync + 'static,
{
    let stack = stack_of(request)?;
    let commit = refresh(syncing).await?;
    let reconcile = ReconcileRequest::new(commit.as_str());
    let reconcile = match &stack {
        Some(name) => reconcile.only(name.as_str()),
        None => reconcile,
    };
    Ok(Json(converge_all(&syncing.shared, hosts, &reconcile).await))
}

async fn sync_all<Up>(
    State(syncing): State<Syncing<Up>>,
    Json(request): Json<SyncRequest>,
) -> Result<Json<SyncResponse>, Refusal>
where
    Up: Upstream + Send + Sync + 'static,
{
    let hosts = syncing.shared.agents.keys().cloned().collect();
    run(&syncing, hosts, &request).await
}

async fn sync_host<Up>(
    State(syncing): State<Syncing<Up>>,
    Path(name): Path<String>,
    Json(request): Json<SyncRequest>,
) -> Result<Json<SyncResponse>, Refusal>
where
    Up: Upstream + Send + Sync + 'static,
{
    let host =
        HostName::new(name).map_err(|error| Refusal::new(StatusCode::BAD_REQUEST, &error))?;
    let mapped = syncing.shared.agents.contains_key(&host);
    let found = mapped.then_some(()).ok_or_else(|| {
        Refusal::new(
            StatusCode::NOT_FOUND,
            &"the controller has no agent for the host",
        )
    });
    found?;
    run(&syncing, vec![host], &request).await
}
