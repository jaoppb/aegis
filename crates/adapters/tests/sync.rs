//! The sync routes and the operator check on a real socket: a real git origin, the polling
//! adapters and a fake agent that records what it is asked.

use std::future::{Future, ready};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aegis_adapters::api::{
    AgentAddress, ApiClient, ClientError, Hub, Operated, enroll_routes, serve_controller_for,
};
use aegis_adapters::{BaseUrl, FsRepo, GitFollower, Remote, SerdeStackParser, SopsCheck};
use aegis_api::version_1::{
    EnrollRequest, HostSync, NODE_PATH, RECONCILE_PATH, ReconcileRequest, SidecarVolume,
    StackState, StackStatus, StatusReport, SyncRequest,
};
use aegis_application::{
    EnrollError, EnrollmentRequest, Grant, PeerAddress, Poller, Reason, Repository,
    SidecarEnroller, WhoIs, WhoIsError,
};
use aegis_domain::{HostName, Login, Logins, NodeLabel, PeerIdentity, TailnetTag};
use axum::extract::State;
use axum::routing::{delete, post};
use axum::{Json, Router};
use reqwest::StatusCode;
use tokio::net::TcpListener;
use tokio::process::Command;

#[cfg(test)]
async fn git(directory: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_DIR")
        .args(["-c", "commit.gpgsign=false", "-c", "user.name=t"])
        .args(["-c", "user.email=t@example.com", "-C"])
        .arg(directory)
        .args(arguments)
        .output()
        .await
        .unwrap();
    assert!(output.status.success(), "git {arguments:?} failed");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[cfg(test)]
const COMPOSE: &str = "services:\n  web:\n    image: nginx:1.27\n";

/// Who calls the controller: a node with a tag, or a person's untagged node.
#[cfg(test)]
#[derive(Clone, Copy)]
enum Caller {
    Tagged(&'static str),
    Person(&'static str),
}

#[cfg(test)]
fn identity_of(caller: Caller) -> PeerIdentity {
    match caller {
        Caller::Tagged(name) => {
            PeerIdentity::new(std::iter::once(TailnetTag::new(name).unwrap()).collect())
        }
        Caller::Person(login) => PeerIdentity::default().with_login(Login::new(login).unwrap()),
    }
}

#[cfg(test)]
struct Peer(Caller);

#[cfg(test)]
impl WhoIs for Peer {
    fn identify(
        &self,
        _peer: &PeerAddress,
    ) -> impl Future<Output = Result<PeerIdentity, WhoIsError>> + Send {
        ready(Ok(identity_of(self.0)))
    }
}

#[cfg(test)]
struct Closed;

#[cfg(test)]
impl SidecarEnroller for Closed {
    fn enroll(
        &self,
        _request: &EnrollmentRequest,
    ) -> impl Future<Output = Result<Grant, EnrollError>> + Send {
        ready(Err(EnrollError::Controller(Reason::new(&"closed"))))
    }
}

#[cfg(test)]
type Calls = Arc<Mutex<Vec<ReconcileRequest>>>;

/// How a fake agent answers: the host it names in its report, how long it takes and whether it
/// fails.
#[cfg(test)]
#[derive(Clone, Copy)]
struct Behaviour {
    host: &'static str,
    delay: Duration,
    mode: Mode,
}

/// Whether a fake agent reports or fails.
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Reports,
    Fails,
}

#[cfg(test)]
const WELL_BEHAVED: Behaviour = Behaviour {
    host: "h",
    delay: Duration::ZERO,
    mode: Mode::Reports,
};

#[cfg(test)]
#[derive(Clone)]
struct Fake {
    calls: Calls,
    behaviour: Behaviour,
}

#[cfg(test)]
async fn answer(
    State(fake): State<Fake>,
    Json(request): Json<ReconcileRequest>,
) -> Result<Json<StatusReport>, StatusCode> {
    fake.calls.lock().unwrap().push(request.clone());
    tokio::time::sleep(fake.behaviour.delay).await;
    if fake.behaviour.mode == Mode::Fails {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let directory = format!("hosts/{}/web", fake.behaviour.host);
    let stack = StackStatus::new(directory, StackState::Running, Vec::new());
    let host = fake.behaviour.host;
    Ok(Json(StatusReport::new(host, request.commit(), vec![stack])))
}

#[cfg(test)]
async fn fake_agent(fake: Fake) -> BaseUrl {
    let app = Router::new()
        .route(RECONCILE_PATH, post(answer))
        .with_state(fake);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = BaseUrl::new(format!("http://{}", listener.local_addr().unwrap()));
    tokio::spawn(async move { axum::serve(listener, app).await });
    base
}

/// A controller for host `h` over a scratch origin, and a client that calls it as `caller`.
#[cfg(test)]
struct World {
    root: PathBuf,
    calls: Calls,
    client: ApiClient,
}

#[cfg(test)]
async fn write_files(origin: &Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let target = origin.join(path);
        let parent = target.parent().unwrap();
        tokio::fs::create_dir_all(parent).await.unwrap();
        tokio::fs::write(target, text).await.unwrap();
    }
}

#[cfg(test)]
impl World {
    fn origin(&self) -> PathBuf {
        self.root.join("origin")
    }

    async fn commit(&self, files: &[(&str, &str)]) -> String {
        write_files(&self.origin(), files).await;
        git(&self.origin(), &["add", "-A"]).await;
        git(&self.origin(), &["commit", "-q", "-m", "change"]).await;
        git(&self.origin(), &["rev-parse", "HEAD"]).await
    }
}

#[cfg(test)]
fn poller(root: &Path) -> Arc<Poller<GitFollower>> {
    let work = root.join("work");
    let remote = Remote::new(root.join("origin").to_str().unwrap(), "trunk").unwrap();
    let git = GitFollower::new(&work, remote);
    let repository = Repository::new(FsRepo::new(&work), SerdeStackParser, SopsCheck);
    Arc::new(Poller::new(git, repository))
}

/// A fresh scratch directory with an empty `origin` repository on the branch the poller follows.
#[cfg(test)]
async fn scratch_origin(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("aegis-sync-{}-{name}", std::process::id()));
    tokio::fs::remove_dir_all(&root).await.ok();
    let origin = root.join("origin");
    tokio::fs::create_dir_all(&origin).await.unwrap();
    git(&origin, &["init", "-q", "-b", "trunk"]).await;
    root
}

/// The agent-only routes of a controller beside the status reports: enrolment, and a stand-in for
/// the deletion of a node.
#[cfg(test)]
fn agent_routes() -> Router {
    let node = delete(|| async { StatusCode::NO_CONTENT });
    enroll_routes(Closed).merge(Router::new().route(NODE_PATH, node))
}

#[cfg(test)]
async fn world(name: &str, caller: Caller, operators: &str) -> World {
    let setup = Setup {
        name,
        caller,
        operators,
    };
    world_with(&setup, &[WELL_BEHAVED]).await
}

/// Which controller to start: its scratch name, who calls it and which logins it admits.
#[cfg(test)]
struct Setup<'setup> {
    name: &'setup str,
    caller: Caller,
    operators: &'setup str,
}

/// One fake agent per behaviour, each listening for the host its behaviour names.
#[cfg(test)]
async fn start_agents(calls: &Calls, agents: &[Behaviour]) -> Vec<AgentAddress> {
    let mut addresses = Vec::new();
    for behaviour in agents {
        let fake = Fake {
            calls: Arc::clone(calls),
            behaviour: *behaviour,
        };
        let host = HostName::new(behaviour.host).unwrap();
        addresses.push(AgentAddress::new(host, fake_agent(fake).await));
    }
    addresses
}

/// A controller with one fake agent per behaviour, each for the host its behaviour names.
#[cfg(test)]
async fn world_with(setup: &Setup<'_>, agents: &[Behaviour]) -> World {
    let root = scratch_origin(setup.name).await;
    let calls = Calls::default();
    let hub = Hub::new(&start_agents(&calls, agents).await).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let logins = Logins::parse_list(setup.operators).unwrap();
    let callers = Operated::new(Peer(setup.caller), logins);
    let routes = hub.grouped(agent_routes(), poller(&root));
    tokio::spawn(serve_controller_for(listener, callers, routes));
    let client = ApiClient::new(BaseUrl::new(url)).unwrap();
    World {
        root,
        calls,
        client,
    }
}

#[cfg(test)]
const fn ada(name: &str) -> Setup<'_> {
    Setup {
        name,
        caller: ADA,
        operators: "ada@example.com",
    }
}

#[cfg(test)]
const ADA: Caller = Caller::Person("ada@example.com");

#[cfg(test)]
const GOOD: [(&str, &str); 1] = [("hosts/h/web/compose.yaml", COMPOSE)];

#[cfg(test)]
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[cfg(test)]
const fn refused<Answer>(result: &Result<Answer, ClientError>) -> Option<StatusCode> {
    match result {
        Err(ClientError::Rejected(status)) => Some(*status),
        _ => None,
    }
}

#[test]
fn a_sync_fetches_now_and_the_agent_reports_what_it_did() {
    runtime().block_on(async {
        let world = world("all", ADA, "ada@example.com").await;
        let commit = world.commit(&GOOD).await;
        let answer = world
            .client
            .sync(None, &SyncRequest::everything())
            .await
            .unwrap();
        assert_eq!(answer.hosts().len(), 1);
        let host = &answer.hosts()[0];
        assert_eq!(host.host(), "h");
        assert_eq!(host.report().unwrap().commit(), commit);
        let calls = world.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].stack(), None);
    });
}

#[test]
fn a_sync_of_one_stack_passes_the_stack_down_and_leaves_the_host_unconfirmed() {
    runtime().block_on(async {
        let world = world("stack", ADA, "ada@example.com").await;
        world.commit(&GOOD).await;
        let host = HostName::new("h").unwrap();
        let request = SyncRequest::only("web");
        world.client.sync(Some(&host), &request).await.unwrap();
        assert_eq!(world.calls.lock().unwrap()[0].stack(), Some("web"));
        let status = world.client.controller_status().await.unwrap();
        let listed = status.hosts().iter().find(|each| each.host() == "h");
        assert!(listed.unwrap().report().is_none());
    });
}

#[test]
fn a_sync_of_a_whole_host_counts_as_its_confirmation() {
    runtime().block_on(async {
        let world = world("confirm", ADA, "ada@example.com").await;
        world.commit(&GOOD).await;
        let host = HostName::new("h").unwrap();
        world
            .client
            .sync(Some(&host), &SyncRequest::everything())
            .await
            .unwrap();
        let status = world.client.controller_status().await.unwrap();
        let listed = status.hosts().iter().find(|each| each.host() == "h");
        assert!(listed.unwrap().report().is_some());
    });
}

#[test]
fn a_host_without_an_agent_is_not_found_and_a_bad_stack_is_a_bad_request() {
    runtime().block_on(async {
        let world = world("refusals", ADA, "ada@example.com").await;
        world.commit(&GOOD).await;
        let nobody = HostName::new("nobody").unwrap();
        let missing = world
            .client
            .sync(Some(&nobody), &SyncRequest::everything())
            .await;
        assert_eq!(refused(&missing), Some(StatusCode::NOT_FOUND));
        let bad = world.client.sync(None, &SyncRequest::only("../etc")).await;
        assert_eq!(refused(&bad), Some(StatusCode::BAD_REQUEST));
        assert!(world.calls.lock().unwrap().is_empty());
    });
}

#[test]
fn a_remote_that_cannot_be_fetched_is_a_bad_gateway_and_nothing_is_asked_of_agents() {
    runtime().block_on(async {
        let world = world("offline", ADA, "ada@example.com").await;
        tokio::fs::remove_dir_all(world.origin()).await.unwrap();
        let result = world.client.sync(None, &SyncRequest::everything()).await;
        assert_eq!(refused(&result), Some(StatusCode::BAD_GATEWAY));
        assert!(world.calls.lock().unwrap().is_empty());
    });
}

#[test]
fn an_unreadable_commit_is_a_conflict_and_nothing_is_asked_of_agents() {
    runtime().block_on(async {
        let world = world("unreadable", ADA, "ada@example.com").await;
        world.commit(&[("hosts", "now a file")]).await;
        let result = world.client.sync(None, &SyncRequest::everything()).await;
        assert_eq!(refused(&result), Some(StatusCode::CONFLICT));
        assert!(world.calls.lock().unwrap().is_empty());
    });
}

/// How the routes of an operator answer: the status read, the sync and the health check.
#[cfg(test)]
async fn operator_routes(world: &World) -> [Option<StatusCode>; 3] {
    let read = refused(&world.client.controller_status().await);
    let sync = refused(&world.client.sync(None, &SyncRequest::everything()).await);
    let health = refused(&world.client.health().await);
    [read, sync, health]
}

/// How the routes of an agent answer: the report, the enrolment and the deletion of a node.
#[cfg(test)]
async fn agent_only_routes(world: &World) -> [Option<StatusCode>; 3] {
    let report = StatusReport::new("h", "abc1234", Vec::new());
    let posted = refused(&world.client.report_status(&report).await);
    let request = EnrollRequest::new("x", vec!["tag:app".to_owned()], SidecarVolume::Empty);
    let enroll = refused(&world.client.enroll(&request).await);
    let label = NodeLabel::new("x").unwrap();
    let node = refused(&world.client.delete_node(&label).await);
    [posted, enroll, node]
}

/// How each route answers `caller`: the status read, the sync, the health check, the report of an
/// agent, the enrolment of a sidecar and the deletion of a node.
#[cfg(test)]
async fn statuses_for(label: &str, caller: Caller, operators: &str) -> [Option<StatusCode>; 6] {
    let world = world(&format!("auth-{label}"), caller, operators).await;
    world.commit(&GOOD).await;
    let [read, sync, health] = operator_routes(&world).await;
    let [posted, enroll, node] = agent_only_routes(&world).await;
    [read, sync, health, posted, enroll, node]
}

#[test]
fn an_operator_reads_syncs_and_checks_health_but_cannot_report_enroll_or_delete_nodes() {
    runtime().block_on(async {
        let denied = Some(StatusCode::FORBIDDEN);
        let seen = statuses_for("operator", ADA, "ada@example.com").await;
        assert_eq!(seen, [None, None, None, denied, denied, denied]);
    });
}

#[test]
fn an_agent_does_everything_but_sync() {
    runtime().block_on(async {
        let agent = Caller::Tagged("tag:aegis-agent");
        let seen = statuses_for("agent", agent, "ada@example.com").await;
        let [read, sync, health, posted, enroll, node] = seen;
        assert_eq!(sync, Some(StatusCode::FORBIDDEN));
        assert_eq!([read, health, node], [None, None, None]);
        assert_ne!(posted, Some(StatusCode::FORBIDDEN));
        assert_ne!(enroll, Some(StatusCode::FORBIDDEN));
    });
}

#[test]
fn a_person_who_is_not_listed_is_refused_everywhere() {
    runtime().block_on(async {
        let stranger = Caller::Person("eve@example.com");
        let seen = statuses_for("stranger", stranger, "ada@example.com").await;
        assert_eq!(seen, [Some(StatusCode::FORBIDDEN); 6]);
    });
}

#[test]
fn with_no_operators_configured_even_a_known_person_is_refused() {
    runtime().block_on(async {
        let seen = statuses_for("nobody", ADA, "").await;
        assert_eq!(seen, [Some(StatusCode::FORBIDDEN); 6]);
    });
}

#[test]
fn two_syncs_of_one_stack_in_a_row_both_run_because_the_host_is_freed() {
    runtime().block_on(async {
        let world = world("twice", ADA, "ada@example.com").await;
        world.commit(&GOOD).await;
        let host = HostName::new("h").unwrap();
        let request = SyncRequest::only("web");
        let first = world.client.sync(Some(&host), &request).await.unwrap();
        let second = world.client.sync(Some(&host), &request).await.unwrap();
        assert!(first.hosts()[0].failure().is_none());
        assert!(second.hosts()[0].failure().is_none());
        assert_eq!(world.calls.lock().unwrap().len(), 2);
    });
}

#[test]
fn an_agent_that_answers_an_error_is_a_failed_host_in_a_successful_sync() {
    runtime().block_on(async {
        let broken = Behaviour {
            mode: Mode::Fails,
            ..WELL_BEHAVED
        };
        let world = world_with(&ada("broken"), &[broken]).await;
        world.commit(&GOOD).await;
        let answer = world
            .client
            .sync(None, &SyncRequest::everything())
            .await
            .unwrap();
        assert!(answer.hosts()[0].report().is_none());
        assert_eq!(
            answer.hosts()[0].failure(),
            Some("the agent answered 500 Internal Server Error")
        );
    });
}

#[test]
fn one_failing_host_does_not_hide_the_other_and_hosts_come_in_name_order() {
    runtime().block_on(async {
        let broken = Behaviour {
            host: "g",
            mode: Mode::Fails,
            ..WELL_BEHAVED
        };
        let world = world_with(&ada("partial"), &[WELL_BEHAVED, broken]).await;
        world.commit(&GOOD).await;
        let answer = world
            .client
            .sync(None, &SyncRequest::everything())
            .await
            .unwrap();
        let names: Vec<&str> = answer.hosts().iter().map(HostSync::host).collect();
        assert_eq!(names, ["g", "h"]);
        assert!(answer.hosts()[0].failure().is_some());
        assert!(answer.hosts()[1].report().is_some());
    });
}

/// Start a sync of everything in a task of its own.
#[cfg(test)]
fn start_sync(
    world: &World,
) -> tokio::task::JoinHandle<Result<aegis_api::version_1::SyncResponse, ClientError>> {
    let client = world.client.clone();
    tokio::spawn(async move { client.sync(None, &SyncRequest::everything()).await })
}

/// Wait until a fake agent has been asked something.
#[cfg(test)]
async fn wait_for_a_call(world: &World) {
    while world.calls.lock().unwrap().is_empty() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[test]
fn a_sync_while_the_host_is_reconciling_says_so_instead_of_queueing() {
    runtime().block_on(async {
        let slow = Behaviour {
            delay: Duration::from_millis(600),
            ..WELL_BEHAVED
        };
        let world = world_with(&ada("busy"), &[slow]).await;
        world.commit(&GOOD).await;
        let running = start_sync(&world);
        wait_for_a_call(&world).await;
        let second = world
            .client
            .sync(None, &SyncRequest::everything())
            .await
            .unwrap();
        let reason = second.hosts()[0].failure().unwrap();
        assert!(reason.contains("already running"), "{reason}");
        assert!(
            running.await.unwrap().unwrap().hosts()[0]
                .report()
                .is_some()
        );
    });
}
