//! The controller's side of the API: `POST /v1/status` and `GET /v1/status`, and the loop that
//! polls the repository and tells the agents to converge.
//!
//! The controller keeps no state on disk. Its view is derived: a restart looks at the repository
//! again and tells every agent again, and a reconcile is idempotent.

use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use aegis_api::version_1::{
    Agent, Confirmation, ControllerStatus, HostStatus, InvalidCommit, ReconcileRequest,
    STATUS_PATH, StatusReport, TargetStatus,
};
use aegis_application::{Fleet, Goal, Invalid, Notice, Observation, Order, Poller, Upstream};
use aegis_domain::{CommitSha, HostName, HostNames};
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};

use super::wire::rejection_status;
use super::{ApiClient, ClientError};
use crate::http::BaseUrl;

/// Where the agent of one host listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAddress {
    host: HostName,
    base: BaseUrl,
}

impl AgentAddress {
    /// The agent of `host` listens at `base`.
    #[must_use]
    pub const fn new(host: HostName, base: BaseUrl) -> Self {
        Self { host, base }
    }
}

/// What the controller knows and may change from several tasks.
struct Book {
    fleet: Fleet,
    reports: BTreeMap<HostName, StatusReport>,
    flying: BTreeSet<HostName>,
    failures: BTreeMap<HostName, String>,
}

impl Book {
    /// Keep what an agent reported, and count it as a confirmation when it is for the target.
    fn record(&mut self, verified: Verified) {
        self.fleet.acknowledge(&verified.host, &verified.commit);
        self.failures.remove(&verified.host);
        self.reports.insert(verified.host, verified.report);
    }

    /// The reconcile call to `host` ended: either its report counts, or the reason is kept.
    fn settle(&mut self, host: &HostName, outcome: Result<Verified, String>) {
        self.flying.remove(host);
        match outcome {
            Ok(verified) => self.record(verified),
            Err(reason) => drop(self.failures.insert(host.clone(), reason)),
        }
    }
}

/// The shared state: the book and the agent clients, by host name.
struct Shared {
    book: Mutex<Book>,
    agents: BTreeMap<HostName, ApiClient>,
}

impl Shared {
    fn book(&self) -> MutexGuard<'_, Book> {
        self.book.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn mapped(&self) -> HostNames {
        self.agents.keys().cloned().collect()
    }

    /// A report about a known host and a well-formed commit.
    fn verify(&self, report: StatusReport) -> Result<Verified, StatusCode> {
        let host = HostName::new(report.host()).map_err(|_| StatusCode::BAD_REQUEST)?;
        let commit = CommitSha::new(report.commit()).map_err(|_| StatusCode::BAD_REQUEST)?;
        let known = self.agents.contains_key(&host);
        known.then_some(()).ok_or(StatusCode::NOT_FOUND)?;
        Ok(Verified {
            host,
            commit,
            report,
        })
    }

    fn snapshot(&self) -> ControllerStatus {
        let book = self.book();
        let target = book.fleet.goal().map(target_status);
        let status = ControllerStatus::new(target, self.host_statuses(&book));
        match book.fleet.notice() {
            Notice::Clear => status,
            Notice::Unloadable(found) => status.with_invalid(invalid_commit(found)),
            Notice::Unreachable(reason) => status.with_fetch_error(reason.to_string()),
        }
    }

    fn host_statuses(&self, book: &Book) -> Vec<HostStatus> {
        let declared = book.fleet.goal().into_iter().flat_map(declared_hosts);
        let mut names: BTreeSet<HostName> = declared.collect();
        names.extend(self.agents.keys().cloned());
        let statuses = names.iter().map(|host| self.host_status(book, host));
        statuses.collect()
    }

    fn host_status(&self, book: &Book, host: &HostName) -> HostStatus {
        let agent = match self.agents.get(host) {
            Some(_) => Agent::Mapped,
            None => Agent::Unmapped,
        };
        let confirmation = match book.failures.get(host) {
            _ if book.fleet.is_confirmed(host) => Confirmation::Confirmed,
            Some(reason) => Confirmation::Failing(reason.clone()),
            None => Confirmation::Pending,
        };
        let status = HostStatus::new(host.as_str(), agent, confirmation);
        match book.reports.get(host) {
            Some(report) => status.with_report(report.clone()),
            None => status,
        }
    }
}

/// A report that passed the checks, with the host and commit it names.
struct Verified {
    host: HostName,
    commit: CommitSha,
    report: StatusReport,
}

/// Keep what an agent reported, and count it as a confirmation when it is for the target.
fn accept(shared: &Shared, report: StatusReport) -> Result<(), StatusCode> {
    let verified = shared.verify(report)?;
    shared.book().record(verified);
    Ok(())
}

/// What a reconcile answer is worth: a report about the host that was asked, or why not.
fn judge(
    shared: &Shared,
    host: &HostName,
    answer: Result<StatusReport, ClientError>,
) -> Result<Verified, String> {
    let report = answer.map_err(|error| error.to_string())?;
    let asked = report.host() == host.as_str();
    asked
        .then_some(())
        .ok_or_else(|| "the agent reported another host".to_owned())?;
    shared.verify(report).map_err(|status| status.to_string())
}

/// An agent answered a reconcile, or failed to: the call is over, and one lock records it.
fn finish(shared: &Shared, host: &HostName, answer: Result<StatusReport, ClientError>) {
    let outcome = judge(shared, host, answer);
    shared.book().settle(host, outcome);
}

fn invalid_commit(found: &Invalid) -> InvalidCommit {
    let details = found.violations().iter().map(ToString::to_string);
    InvalidCommit::new(found.commit().as_str(), details.collect())
}

fn declared_hosts(goal: &Goal) -> impl Iterator<Item = HostName> + '_ {
    let hosts = goal.outcome().state().hosts().iter();
    hosts.map(|host| host.name().clone())
}

fn target_status(goal: &Goal) -> TargetStatus {
    let rejected = goal.outcome().rejected().iter().map(rejection_status);
    TargetStatus::new(goal.commit().as_str(), rejected.collect())
}

/// One reconcile call of one host, run in its own task, so a slow deploy on one host delays
/// neither the others nor the next poll.
struct Call {
    shared: Arc<Shared>,
    client: ApiClient,
    host: HostName,
    request: ReconcileRequest,
}

impl Call {
    async fn run(self) {
        let answer = self.client.reconcile(&self.request).await;
        finish(&self.shared, &self.host, answer);
    }
}

/// Start the call to `host` unless one is already running or the host has no agent. A call for an
/// older commit that is still running holds the host back until it ends.
fn launch(shared: &Arc<Shared>, host: &HostName, commit: &CommitSha) {
    let Some(client) = shared.agents.get(host).cloned() else {
        return;
    };
    if !shared.book().flying.insert(host.clone()) {
        return;
    }
    let call = Call {
        shared: Arc::clone(shared),
        client,
        host: host.clone(),
        request: ReconcileRequest::new(commit.as_str()),
    };
    drop(tokio::spawn(call.run()));
}

/// Start telling every host of `order` to converge.
fn converge(shared: &Arc<Shared>, order: &Order) {
    let hosts = order.hosts().iter();
    hosts.for_each(|host| launch(shared, host, order.commit()));
}

async fn receive(
    State(shared): State<Arc<Shared>>,
    Json(report): Json<StatusReport>,
) -> StatusCode {
    accept(&shared, report).map_or_else(|status| status, |()| StatusCode::NO_CONTENT)
}

async fn snapshot(State(shared): State<Arc<Shared>>) -> Json<ControllerStatus> {
    Json(shared.snapshot())
}

/// The controller: what it knows of the repository and the fleet, and the way it tells agents.
pub struct Hub {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Hub {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Hub")
    }
}

impl Hub {
    /// A controller that can reach the agents at `addresses`.
    ///
    /// # Errors
    ///
    /// Returns a [`ClientError`] when an HTTP client cannot be built.
    pub fn new(addresses: &[AgentAddress]) -> Result<Self, ClientError> {
        let ready = addresses.iter().map(client_for);
        let ready = ready.collect::<Result<Vec<_>, _>>()?;
        let clients: Clients = ready.into_iter().collect();
        let book = Book {
            fleet: Fleet::default(),
            reports: BTreeMap::new(),
            flying: BTreeSet::new(),
            failures: BTreeMap::new(),
        };
        let shared = Shared {
            book: Mutex::new(book),
            agents: clients.0,
        };
        Ok(Self {
            shared: Arc::new(shared),
        })
    }

    /// `POST /v1/status` and `GET /v1/status`, to be served behind the tag check (which answers
    /// `403` or `503`). `POST` answers `204`, `400` for a malformed host or commit and `404` for a
    /// host that has no agent address; `GET` answers a [`ControllerStatus`].
    pub fn routes(&self) -> Router {
        let methods = get(snapshot).post(receive);
        Router::new()
            .route(STATUS_PATH, methods)
            .with_state(Arc::clone(&self.shared))
    }

    /// Look at the repository once and tell the hosts that have not confirmed the newest good
    /// commit to converge to it. A failed fetch or an unreadable commit changes nothing that runs.
    pub async fn tick<Up: Upstream + Sync>(&self, poller: &Poller<Up>) {
        let observation = poller.observe().await;
        let order = plan(&self.shared, observation);
        tell(&self.shared, order);
    }

    /// [`Hub::tick`] every `interval`, forever.
    pub async fn run<Up: Upstream + Sync>(
        &self,
        poller: &Poller<Up>,
        interval: Duration,
    ) -> Infallible {
        forever(self, poller, interval).await
    }
}

fn tell(shared: &Arc<Shared>, order: Option<Order>) {
    if let Some(due) = order {
        converge(shared, &due);
    }
}

/// The client of one agent, ready to be kept under its host name.
struct Ready {
    host: HostName,
    client: ApiClient,
}

/// The clients of every agent, by host name.
struct Clients(BTreeMap<HostName, ApiClient>);

impl FromIterator<Ready> for Clients {
    fn from_iter<Source: IntoIterator<Item = Ready>>(source: Source) -> Self {
        let entries = source.into_iter().map(|ready| (ready.host, ready.client));
        Self(entries.collect())
    }
}

fn client_for(address: &AgentAddress) -> Result<Ready, ClientError> {
    let client = ApiClient::new(address.base.clone())?;
    Ok(Ready {
        host: address.host.clone(),
        client,
    })
}

/// Take in what the repository looked like, and say whom to tell about the goal.
fn plan(shared: &Arc<Shared>, observation: Observation) -> Option<Order> {
    let mapped = shared.mapped();
    let mut book = shared.book();
    book.fleet.observe(observation);
    book.fleet.order(&mapped)
}

async fn forever<Up: Upstream + Sync>(
    hub: &Hub,
    poller: &Poller<Up>,
    interval: Duration,
) -> Infallible {
    loop {
        hub.tick(poller).await;
        tokio::time::sleep(interval).await;
    }
}
