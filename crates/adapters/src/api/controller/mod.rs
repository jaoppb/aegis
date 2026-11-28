//! The controller's side of the API: `POST /v1/status` and `GET /v1/status`, and the loop that
//! polls the repository and tells the agents to converge.
//!
//! The controller keeps no state on disk. Its view is derived: a restart looks at the repository
//! again and tells every agent again, and a reconcile is idempotent.

mod sync;

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
use axum::routing::{get, post};
use axum::{Json, Router};

use super::Routes;
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

    /// The call to `host` is over: its `outcome` is recorded like a reconcile of the poll, or
    /// there is none to record (the report was for one stack) and the host is only freed.
    fn conclude(&mut self, host: &HostName, outcome: Option<Result<Verified, String>>) {
        match outcome {
            Some(answer) => self.settle(host, answer),
            None => self.release(host),
        }
    }

    /// The call to `host` is over, and a new one may start.
    fn release(&mut self, host: &HostName) {
        self.flying.remove(host);
    }

    /// The reconcile call to `host` ended: either its report counts, or the reason is kept.
    fn settle(&mut self, host: &HostName, outcome: Result<Verified, String>) {
        self.release(host);
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
    /// Held while the repository is fetched, so a poll and a sync never fetch at the same time.
    fetching: tokio::sync::Mutex<()>,
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
        let confirmation = confirmation_of(book, host);
        let status = HostStatus::new(host.as_str(), agent, confirmation);
        match book.reports.get(host) {
            Some(report) => status.with_report(report.clone()),
            None => status,
        }
    }
}

/// Whether `host` confirmed the target, and if not, whether the last call to it failed.
fn confirmation_of(book: &Book, host: &HostName) -> Confirmation {
    if book.fleet.is_confirmed(host) {
        return Confirmation::Confirmed;
    }
    let failure = book.failures.get(host);
    failure.map_or(Confirmation::Pending, |reason| {
        Confirmation::Failing(reason.clone())
    })
}

/// A report that passed the checks, with the host and commit it names.
struct Verified {
    host: HostName,
    commit: CommitSha,
    report: StatusReport,
}

/// Verify a report an agent pushed, then record it.
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
    let report = answer.map_err(|error| describe(&error))?;
    let asked = report.host() == host.as_str();
    asked
        .then_some(())
        .ok_or_else(|| "the agent reported another host".to_owned())?;
    shared.verify(report).map_err(|status| status.to_string())
}

/// Why a call failed, without the agent's URL: the status is readable by every agent.
fn describe(error: &ClientError) -> String {
    match error {
        ClientError::Rejected(status) => format!("the agent answered {status}"),
        ClientError::Transport(cause) if cause.is_timeout() => "the agent timed out".to_owned(),
        ClientError::Transport(cause) if cause.is_connect() => {
            "the agent is unreachable".to_owned()
        }
        ClientError::Transport(_) => "the call to the agent failed".to_owned(),
    }
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

/// What came of asking the agent of a host to reconcile.
enum Asked {
    /// The agent was not called, for this reason.
    Skipped(&'static str),
    /// The agent answered, or the call failed: what the answer is worth.
    Answered(Result<Verified, String>),
}

/// Ask the agent of `host` to reconcile `request`, unless the host has no agent or a call to it is
/// already running. The host stays claimed until the caller records the answer with
/// [`Book::settle`] or frees it with [`Book::release`].
async fn ask(shared: &Shared, host: &HostName, request: &ReconcileRequest) -> Asked {
    let Some(client) = shared.agents.get(host) else {
        return Asked::Skipped("the host has no agent");
    };
    if !shared.book().flying.insert(host.clone()) {
        return Asked::Skipped("a reconcile is already running on this host");
    }
    let answer = client.reconcile(request).await;
    Asked::Answered(judge(shared, host, answer))
}

/// One reconcile call of the poll, run in its own task, so a slow deploy on one host delays neither
/// the others nor the next poll.
async fn poll_call(shared: Arc<Shared>, host: HostName, request: ReconcileRequest) {
    if let Asked::Answered(outcome) = ask(&shared, &host, &request).await {
        shared.book().settle(&host, outcome);
    }
}

/// Start the call to `host` unless one is already running or the host has no agent. A call for an
/// older commit that is still running holds the host back until it ends.
fn launch(shared: &Arc<Shared>, host: &HostName, commit: &CommitSha) {
    let request = ReconcileRequest::new(commit.as_str());
    drop(tokio::spawn(poll_call(
        Arc::clone(shared),
        host.clone(),
        request,
    )));
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
            fetching: tokio::sync::Mutex::new(()),
        };
        Ok(Self {
            shared: Arc::new(shared),
        })
    }

    /// `POST /v1/status` and `GET /v1/status`, to be served behind the tag check (which answers
    /// `403` or `503`). `POST` answers `204`, `400` for a malformed host or commit and `404` for a
    /// host that has no agent address; `GET` answers a [`ControllerStatus`].
    pub fn routes(&self) -> Router {
        self.read_routes().merge(self.report_routes())
    }

    /// `GET /v1/status` alone: the status an operator reads, answering a [`ControllerStatus`].
    pub fn read_routes(&self) -> Router {
        Router::new()
            .route(STATUS_PATH, get(snapshot))
            .with_state(Arc::clone(&self.shared))
    }

    /// `POST /v1/status` alone: the reports of agents, answering `204`, `400` for a malformed
    /// host or commit and `404` for a host that has no agent address.
    pub fn report_routes(&self) -> Router {
        Router::new()
            .route(STATUS_PATH, post(receive))
            .with_state(Arc::clone(&self.shared))
    }

    /// Every route of a controller, grouped by who may call it: the `agents` routes (and the
    /// reports of agents) are for the tagged peer alone, `GET /v1/status` is for an agent or an
    /// operator, and the sync routes, which look at the repository with `poller`, are for an
    /// operator alone.
    pub fn grouped<Up>(&self, agents: Router, poller: Arc<Poller<Up>>) -> Routes
    where
        Up: Upstream + Send + Sync + 'static,
    {
        Routes::default()
            .tagged(self.report_routes().merge(agents))
            .either(self.read_routes())
            .operators(self.sync_routes(poller))
    }

    /// The sync routes, which look at the repository with `poller` when asked and make the agents
    /// converge: `POST /v1/syncs` for every host and `POST /v1/hosts/{host}/syncs` for one. They
    /// are to be served behind the operator check (which answers `403` or `503`); the answers are
    /// listed with [`aegis_api::version_1::SYNCS_PATH`].
    pub fn sync_routes<Up>(&self, poller: Arc<Poller<Up>>) -> Router
    where
        Up: Upstream + Send + Sync + 'static,
    {
        sync::routes(Arc::clone(&self.shared), poller)
    }

    /// Look at the repository once and tell the hosts that have not confirmed the newest good
    /// commit to converge to it. A failed fetch or an unreadable commit changes nothing that runs.
    pub async fn tick<Up: Upstream + Sync>(&self, poller: &Poller<Up>) {
        let order = look(&self.shared, poller).await;
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

/// Tell the hosts of `order`, when there is one, to converge.
fn tell(shared: &Arc<Shared>, order: Option<Order>) {
    if let Some(due) = order {
        converge(shared, &due);
    }
}

/// Fetch and load the repository, never at the same time as another look, and say whom to tell
/// about the goal it found.
async fn look<Up: Upstream + Sync>(shared: &Arc<Shared>, poller: &Poller<Up>) -> Option<Order> {
    let _fetching = shared.fetching.lock().await;
    let observation = poller.observe().await;
    plan(shared, observation)
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
