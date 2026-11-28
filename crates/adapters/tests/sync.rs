//! The sync routes and the operator check on a real socket: a real git origin, the polling
//! adapters and a fake agent that records what it is asked.

use std::future::{Future, ready};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use aegis_adapters::api::{
    AgentAddress, ApiClient, ClientError, Hub, Operated, Routes, enroll_routes,
    serve_controller_for,
};
use aegis_adapters::{BaseUrl, FsRepo, GitFollower, Remote, SerdeStackParser, SopsCheck};
use aegis_api::version_1::{
    EnrollRequest, RECONCILE_PATH, ReconcileRequest, SidecarVolume, StackState, StackStatus,
    StatusReport, SyncRequest,
};
use aegis_application::{
    EnrollError, EnrollmentRequest, Grant, PeerAddress, Poller, Reason, Repository,
    SidecarEnroller, WhoIs, WhoIsError,
};
use aegis_domain::{HostName, Login, Logins, PeerIdentity, TailnetTag};
use axum::extract::State;
use axum::routing::post;
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

#[cfg(test)]
async fn answer(
    State(calls): State<Calls>,
    Json(request): Json<ReconcileRequest>,
) -> Json<StatusReport> {
    calls.lock().unwrap().push(request.clone());
    let stack = StackStatus::new("hosts/h/web", StackState::Running, Vec::new());
    Json(StatusReport::new("h", request.commit(), vec![stack]))
}

#[cfg(test)]
async fn fake_agent(calls: Calls) -> BaseUrl {
    let app = Router::new()
        .route(RECONCILE_PATH, post(answer))
        .with_state(calls);
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

/// The routes of a controller as `aegis-controller` groups them by caller.
#[cfg(test)]
fn grouped(hub: &Hub, root: &Path) -> Routes {
    Routes::default()
        .tagged(hub.report_routes().merge(enroll_routes(Closed)))
        .either(hub.read_routes())
        .operators(hub.sync_routes(poller(root)))
}

#[cfg(test)]
async fn world(name: &str, caller: Caller, operators: &str) -> World {
    let root = scratch_origin(name).await;
    let calls = Calls::default();
    let agent = fake_agent(Arc::clone(&calls)).await;
    let address = AgentAddress::new(HostName::new("h").unwrap(), agent);
    let hub = Hub::new(&[address]).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let callers = Operated::new(Peer(caller), Logins::parse_list(operators).unwrap());
    tokio::spawn(serve_controller_for(
        listener,
        callers,
        grouped(&hub, &root),
    ));
    let client = ApiClient::new(BaseUrl::new(url)).unwrap();
    World {
        root,
        calls,
        client,
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

/// How each route answers `caller`: the status read, the sync, the report of an agent and the
/// enrolment of a sidecar.
#[cfg(test)]
async fn statuses_for(caller: Caller, operators: &str) -> [Option<StatusCode>; 4] {
    let name = format!("auth-{}", rand_name());
    let world = world(&name, caller, operators).await;
    world.commit(&GOOD).await;
    let read = refused(&world.client.controller_status().await);
    let sync = refused(&world.client.sync(None, &SyncRequest::everything()).await);
    let report = StatusReport::new("h", "abc1234", Vec::new());
    let posted = refused(&world.client.report_status(&report).await);
    let request = EnrollRequest::new("x", vec!["tag:app".to_owned()], SidecarVolume::Empty);
    let enroll = refused(&world.client.enroll(&request).await);
    [read, sync, posted, enroll]
}

#[cfg(test)]
fn rand_name() -> String {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed).to_string()
}

#[test]
fn an_operator_reads_and_syncs_but_cannot_report_or_enroll() {
    runtime().block_on(async {
        let forbidden = Some(StatusCode::FORBIDDEN);
        let seen = statuses_for(ADA, "ada@example.com").await;
        assert_eq!(seen[0], None);
        assert_eq!(seen[1], None);
        assert_eq!(seen[2], forbidden);
        assert_eq!(seen[3], forbidden);
    });
}

#[test]
fn an_agent_reads_reports_and_enrolls_but_cannot_sync() {
    runtime().block_on(async {
        let agent = Caller::Tagged("tag:aegis-agent");
        let seen = statuses_for(agent, "ada@example.com").await;
        assert_eq!(seen[0], None);
        assert_eq!(seen[1], Some(StatusCode::FORBIDDEN));
        assert_ne!(seen[2], Some(StatusCode::FORBIDDEN));
        assert_ne!(seen[3], Some(StatusCode::FORBIDDEN));
    });
}

#[test]
fn a_person_who_is_not_listed_is_refused_everywhere() {
    runtime().block_on(async {
        let stranger = Caller::Person("eve@example.com");
        let forbidden = Some(StatusCode::FORBIDDEN);
        let seen = statuses_for(stranger, "ada@example.com").await;
        assert_eq!(seen, [forbidden; 4]);
    });
}

#[test]
fn with_no_operators_configured_even_a_known_person_is_refused() {
    runtime().block_on(async {
        let forbidden = Some(StatusCode::FORBIDDEN);
        assert_eq!(statuses_for(ADA, "").await, [forbidden; 4]);
    });
}
