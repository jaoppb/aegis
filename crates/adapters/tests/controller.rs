//! The controller end to end: a real git origin, the polling adapters, the status routes on a real
//! socket and a fake agent that answers `POST /v1/reconcile`.

use std::future::{Future, ready};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aegis_adapters::api::{
    AgentAddress, ApiClient, ClientError, Hub, enroll_routes, serve_controller,
};
use aegis_adapters::{BaseUrl, FsRepo, GitFollower, Remote, SerdeStackParser, SopsCheck};
use aegis_api::version_1::{
    Agent as Wire, Confirmation, ControllerStatus, EnrollRequest, HostStatus, RECONCILE_PATH,
    ReconcileRequest, STATUS_PATH, SidecarVolume, StackState, StackStatus, StatusReport,
};
use aegis_application::{
    EnrollError, EnrollmentRequest, Grant, PeerAddress, Poller, Reason, Repository,
    SidecarEnroller, WhoIs, WhoIsError,
};
use aegis_domain::{HostName, PeerIdentity, TailnetTag};
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

/// A scratch directory with an `origin` repository and a path for the controller's work dir.
#[cfg(test)]
struct World {
    root: PathBuf,
    calls: Arc<Mutex<Vec<String>>>,
    hub: Hub,
    url: String,
}

#[cfg(test)]
impl World {
    fn origin(&self) -> PathBuf {
        self.root.join("origin")
    }

    fn poller(&self) -> Poller<GitFollower> {
        let work = self.root.join("work");
        let remote = Remote::new(self.origin().to_str().unwrap(), "trunk").unwrap();
        let git = GitFollower::new(&work, remote);
        let repository = Repository::new(FsRepo::new(&work), SerdeStackParser, SopsCheck);
        Poller::new(git, repository)
    }

    async fn commit(&self, files: &[(&str, &str)]) -> String {
        write_files(&self.origin(), files).await;
        git(&self.origin(), &["add", "-A"]).await;
        git(&self.origin(), &["commit", "-q", "-m", "change"]).await;
        git(&self.origin(), &["rev-parse", "HEAD"]).await
    }

    async fn status(&self) -> ControllerStatus {
        let client = ApiClient::new(BaseUrl::new(&self.url)).unwrap();
        client.controller_status().await.unwrap()
    }

    /// Wait until the host confirmed the target, as the agent's answer arrives in a task.
    async fn confirmed(&self, host: &str) -> ControllerStatus {
        let client = ApiClient::new(BaseUrl::new(&self.url)).unwrap();
        wait_confirmed(&client, host).await
    }

    async fn failing(&self, host: &str) -> ControllerStatus {
        let client = ApiClient::new(BaseUrl::new(&self.url)).unwrap();
        wait_failing(&client, host).await
    }
}

#[cfg(test)]
async fn write_files(origin: &Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let target = origin.join(path);
        tokio::fs::create_dir_all(target.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(target, text).await.unwrap();
    }
}

#[cfg(test)]
async fn wait_confirmed(client: &ApiClient, host: &str) -> ControllerStatus {
    wait_for(client, host, |found| {
        *found.confirmation() == Confirmation::Confirmed
    })
    .await
}

#[cfg(test)]
async fn wait_failing(client: &ApiClient, host: &str) -> ControllerStatus {
    let failing = |found: &HostStatus| matches!(found.confirmation(), Confirmation::Failing(_));
    wait_for(client, host, failing).await
}

/// Poll the status until `host` satisfies `wanted`: agent calls end in tasks of their own.
#[cfg(test)]
async fn wait_for(
    client: &ApiClient,
    host: &str,
    wanted: impl Fn(&HostStatus) -> bool,
) -> ControllerStatus {
    let mut status = client.controller_status().await.unwrap();
    let mut tries: u32 = 0;
    while !holds(&status, host, &wanted) && tries < 250 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        status = client.controller_status().await.unwrap();
        tries = tries.saturating_add(1);
    }
    assert!(holds(&status, host, &wanted), "{host} never got there");
    status
}

#[cfg(test)]
fn holds(status: &ControllerStatus, host: &str, wanted: &impl Fn(&HostStatus) -> bool) -> bool {
    let found = status.hosts().iter().find(|found| found.host() == host);
    found.is_some_and(wanted)
}

/// How the fake agent answers: it fails the first `failures` calls with a 500, waits `delay`
/// before answering and names `answers_as` as the host in its report.
#[cfg(test)]
#[derive(Clone, Copy)]
struct Behaviour {
    failures: usize,
    delay: Duration,
    answers_as: &'static str,
}

#[cfg(test)]
const WELL_BEHAVED: Behaviour = Behaviour {
    failures: 0,
    delay: Duration::ZERO,
    answers_as: "h",
};

#[cfg(test)]
#[derive(Clone)]
struct Agent {
    calls: Arc<Mutex<Vec<String>>>,
    behaviour: Behaviour,
}

#[cfg(test)]
async fn answer(
    State(agent): State<Agent>,
    Json(request): Json<ReconcileRequest>,
) -> Result<Json<StatusReport>, StatusCode> {
    let number = {
        let mut calls = agent.calls.lock().unwrap();
        calls.push(request.commit().to_owned());
        calls.len()
    };
    tokio::time::sleep(agent.behaviour.delay).await;
    if number <= agent.behaviour.failures {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let stack = StackStatus::new("hosts/h/web", StackState::Running, Vec::new());
    let host = agent.behaviour.answers_as;
    Ok(Json(StatusReport::new(host, request.commit(), vec![stack])))
}

#[cfg(test)]
async fn fake_agent(agent: Agent) -> BaseUrl {
    let app = Router::new()
        .route(RECONCILE_PATH, post(answer))
        .with_state(agent);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = BaseUrl::new(format!("http://{}", listener.local_addr().unwrap()));
    tokio::spawn(async move { axum::serve(listener, app).await });
    base
}

#[cfg(test)]
async fn world(name: &str) -> World {
    world_with(name, WELL_BEHAVED).await
}

#[cfg(test)]
async fn world_with(name: &str, behaviour: Behaviour) -> World {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let agent = Agent {
        calls: calls.clone(),
        behaviour,
    };
    world_over(name, calls, fake_agent(agent).await).await
}

/// A world whose only agent listens at `agent`.
#[cfg(test)]
async fn world_over(name: &str, calls: Arc<Mutex<Vec<String>>>, agent: BaseUrl) -> World {
    let root = std::env::temp_dir().join(format!("aegis-ctl-{}-{name}", std::process::id()));
    tokio::fs::remove_dir_all(&root).await.ok();
    tokio::fs::create_dir_all(root.join("origin"))
        .await
        .unwrap();
    git(&root.join("origin"), &["init", "-q", "-b", "trunk"]).await;
    let address = AgentAddress::new(HostName::new("h").unwrap(), agent);
    let hub = Hub::new(&[address]).unwrap();
    let url = serve(hub.routes()).await;
    World {
        root,
        calls,
        hub,
        url,
    }
}

/// Serve `routes` on a free local port and say where.
#[cfg(test)]
async fn serve(routes: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, routes).await });
    url
}

#[cfg(test)]
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[cfg(test)]
const GOOD: [(&str, &str); 1] = [("hosts/h/web/compose.yaml", COMPOSE)];

#[test]
fn a_new_commit_is_dispatched_to_the_host_and_confirmed() {
    runtime().block_on(async {
        let world = world("dispatch").await;
        let first = world.commit(&GOOD).await;
        world.hub.tick(&world.poller()).await;
        let status = world.confirmed("h").await;
        assert_eq!(status.target().unwrap().commit(), first);
        assert_eq!(
            world.calls.lock().unwrap().as_slice(),
            std::slice::from_ref(&first)
        );
        let second = world.commit(&[("hosts/h/web/note", "x")]).await;
        world.hub.tick(&world.poller()).await;
        let moved = world.confirmed("h").await;
        assert_eq!(moved.target().unwrap().commit(), second);
        assert_eq!(*world.calls.lock().unwrap(), [first, second]);
    });
}

#[test]
fn a_confirmed_host_is_not_told_again_on_the_next_poll() {
    runtime().block_on(async {
        let world = world("quiet").await;
        world.commit(&GOOD).await;
        world.hub.tick(&world.poller()).await;
        world.confirmed("h").await;
        world.hub.tick(&world.poller()).await;
        world.hub.tick(&world.poller()).await;
        assert_eq!(world.calls.lock().unwrap().len(), 1);
    });
}

#[test]
fn a_rejected_stack_is_reported_and_the_commit_is_still_dispatched() {
    runtime().block_on(async {
        let world = world("partial").await;
        let files = [GOOD[0], ("hosts/h/broken/readme", "no compose file")];
        let commit = world.commit(&files).await;
        world.hub.tick(&world.poller()).await;
        let status = world.confirmed("h").await;
        let target = status.target().unwrap();
        assert_eq!(target.commit(), commit);
        assert_eq!(target.rejected().len(), 1);
        assert_eq!(target.rejected()[0].directory(), "hosts/h/broken");
        assert_eq!(target.rejected()[0].state(), StackState::Invalid);
    });
}

#[test]
fn an_unreadable_commit_is_reported_and_the_last_good_one_stays_the_target() {
    runtime().block_on(async {
        let world = world("unreadable").await;
        let good = world.commit(&GOOD).await;
        world.hub.tick(&world.poller()).await;
        world.confirmed("h").await;
        git(&world.origin(), &["rm", "-rq", "hosts"]).await;
        let broken = world.commit(&[("hosts", "now a file")]).await;
        world.hub.tick(&world.poller()).await;
        let status = world.status().await;
        assert_eq!(status.target().unwrap().commit(), good);
        assert_eq!(status.invalid().unwrap().commit(), broken);
        assert!(!status.invalid().unwrap().details().is_empty());
        assert_eq!(*world.calls.lock().unwrap(), [good]);
    });
}

#[test]
fn a_failed_fetch_leaves_the_target_and_the_confirmations_alone() {
    runtime().block_on(async {
        let world = world("offline").await;
        let good = world.commit(&GOOD).await;
        world.hub.tick(&world.poller()).await;
        world.confirmed("h").await;
        tokio::fs::remove_dir_all(world.origin()).await.unwrap();
        world.hub.tick(&world.poller()).await;
        let status = world.status().await;
        assert!(status.fetch_error().is_some());
        assert_eq!(status.target().unwrap().commit(), good);
        assert_eq!(status.hosts()[0].confirmation(), &Confirmation::Confirmed);
        assert_eq!(world.calls.lock().unwrap().len(), 1);
    });
}

#[test]
fn a_host_of_the_repository_without_an_agent_is_listed_as_unmapped() {
    runtime().block_on(async {
        let world = world("unmapped").await;
        world
            .commit(&[GOOD[0], ("hosts/lonely/web/compose.yaml", COMPOSE)])
            .await;
        world.hub.tick(&world.poller()).await;
        world.confirmed("h").await;
        let status = world.status().await;
        let lonely = status.hosts().iter().find(|found| found.host() == "lonely");
        assert_eq!(lonely.unwrap().agent(), Wire::Unmapped);
    });
}

#[cfg(test)]
async fn post_report(world: &World, report: &StatusReport) -> StatusCode {
    let url = format!("{}{STATUS_PATH}", world.url);
    let client = reqwest::Client::new();
    client.post(url).json(report).send().await.unwrap().status()
}

#[test]
fn a_status_report_is_accepted_only_from_a_known_host_with_a_valid_commit() {
    runtime().block_on(async {
        let world = world("intake").await;
        let commit = world.commit(&GOOD).await;
        world.hub.tick(&world.poller()).await;
        world.confirmed("h").await;
        let stranger = StatusReport::new("stranger", commit.as_str(), Vec::new());
        assert_eq!(post_report(&world, &stranger).await, StatusCode::NOT_FOUND);
        let garbled = StatusReport::new("h", "not a commit", Vec::new());
        assert_eq!(post_report(&world, &garbled).await, StatusCode::BAD_REQUEST);
        let genuine = StatusReport::new("h", commit.as_str(), Vec::new());
        assert_eq!(post_report(&world, &genuine).await, StatusCode::NO_CONTENT);
        let kept = world.status().await.hosts()[0].report().cloned();
        assert_eq!(kept, Some(genuine));
    });
}

#[cfg(test)]
async fn restore_hosts(world: &World) -> String {
    git(&world.origin(), &["rm", "-q", "hosts"]).await;
    world.commit(&GOOD).await
}

#[test]
fn the_controller_recovers_once_a_good_commit_follows_an_unreadable_one() {
    runtime().block_on(async {
        let world = world("recover").await;
        world.commit(&GOOD).await;
        git(&world.origin(), &["rm", "-rq", "hosts"]).await;
        world.commit(&[("hosts", "now a file")]).await;
        world.hub.tick(&world.poller()).await;
        assert!(world.status().await.invalid().is_some());
        let fixed = restore_hosts(&world).await;
        world.hub.tick(&world.poller()).await;
        let status = world.confirmed("h").await;
        assert!(status.invalid().is_none());
        assert_eq!(status.target().unwrap().commit(), fixed);
    });
}

#[test]
fn an_unreadable_aegis_directory_makes_the_commit_unreadable_too() {
    runtime().block_on(async {
        let world = world("aegis-root").await;
        let good = world.commit(&GOOD).await;
        world.hub.tick(&world.poller()).await;
        world.confirmed("h").await;
        let broken = world.commit(&[("aegis", "now a file")]).await;
        world.hub.tick(&world.poller()).await;
        let status = world.status().await;
        assert_eq!(status.invalid().unwrap().commit(), broken);
        assert_eq!(status.target().unwrap().commit(), good);
    });
}

#[test]
fn a_host_whose_agent_failed_is_told_again_on_the_next_poll() {
    runtime().block_on(async {
        let behaviour = Behaviour {
            failures: 1,
            ..WELL_BEHAVED
        };
        let world = world_with("retry", behaviour).await;
        world.commit(&GOOD).await;
        world.hub.tick(&world.poller()).await;
        let failed = world.failing("h").await;
        assert_eq!(failed.hosts()[0].report(), None);
        world.hub.tick(&world.poller()).await;
        world.confirmed("h").await;
        assert_eq!(world.calls.lock().unwrap().len(), 2);
    });
}

#[test]
fn an_answer_about_another_host_does_not_confirm_the_asked_host() {
    runtime().block_on(async {
        let behaviour = Behaviour {
            answers_as: "other",
            ..WELL_BEHAVED
        };
        let world = world_with("impostor", behaviour).await;
        world.commit(&GOOD).await;
        world.hub.tick(&world.poller()).await;
        let status = world.failing("h").await;
        assert_eq!(status.hosts()[0].report(), None);
    });
}

#[test]
fn a_host_with_a_call_in_flight_is_not_told_again() {
    runtime().block_on(async {
        let behaviour = Behaviour {
            delay: Duration::from_millis(300),
            ..WELL_BEHAVED
        };
        let world = world_with("flying", behaviour).await;
        world.commit(&GOOD).await;
        world.hub.tick(&world.poller()).await;
        world.hub.tick(&world.poller()).await;
        world.confirmed("h").await;
        world.hub.tick(&world.poller()).await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(world.calls.lock().unwrap().len(), 1);
    });
}

#[test]
fn the_polling_loop_picks_up_each_new_commit_without_a_manual_tick() {
    runtime().block_on(async {
        let world = world("loop").await;
        world.commit(&GOOD).await;
        let poller = world.poller();
        let looping = world.hub.run(&poller, Duration::from_millis(50));
        let second = commit_after_first_confirmation(&world);
        let status = tokio::select! {
            never = looping => match never {},
            status = second => status,
        };
        assert_eq!(world.calls.lock().unwrap().len(), 2);
        assert!(status.target().is_some());
    });
}

/// Wait for the first commit to be confirmed, push another and wait for that one too.
#[cfg(test)]
async fn commit_after_first_confirmation(world: &World) -> ControllerStatus {
    world.confirmed("h").await;
    let next = world.commit(&[("hosts/h/web/note", "x")]).await;
    wait_target(world, &next).await
}

/// Poll until the target is `commit` and its host confirmed it.
#[cfg(test)]
async fn wait_target(world: &World, commit: &str) -> ControllerStatus {
    let client = ApiClient::new(BaseUrl::new(&world.url)).unwrap();
    let at_commit = |found: &HostStatus| {
        let report = found.report();
        report.is_some_and(|seen| seen.commit() == commit)
            && *found.confirmation() == Confirmation::Confirmed
    };
    wait_for(&client, "h", at_commit).await
}

#[cfg(test)]
const NEVER_ANSWERS: Behaviour = Behaviour {
    failures: usize::MAX,
    ..WELL_BEHAVED
};

#[test]
fn a_report_for_an_older_commit_is_kept_but_does_not_confirm_the_host() {
    runtime().block_on(async {
        let world = world_with("stale", NEVER_ANSWERS).await;
        world.commit(&GOOD).await;
        world.hub.tick(&world.poller()).await;
        world.failing("h").await;
        let stale = StatusReport::new("h", "1234567", Vec::new());
        assert_eq!(post_report(&world, &stale).await, StatusCode::NO_CONTENT);
        let status = world.status().await;
        let host = &status.hosts()[0];
        assert_eq!(host.report(), Some(&stale));
        assert_ne!(host.confirmation(), &Confirmation::Confirmed);
    });
}

#[test]
fn refused_reports_are_not_kept() {
    runtime().block_on(async {
        let world = world_with("refused", NEVER_ANSWERS).await;
        let commit = world.commit(&GOOD).await;
        world.hub.tick(&world.poller()).await;
        world.failing("h").await;
        let stranger = StatusReport::new("stranger", commit.as_str(), Vec::new());
        assert_eq!(post_report(&world, &stranger).await, StatusCode::NOT_FOUND);
        let garbled = StatusReport::new("h", "not a commit", Vec::new());
        assert_eq!(post_report(&world, &garbled).await, StatusCode::BAD_REQUEST);
        let status = world.status().await;
        assert_eq!(status.hosts().len(), 1);
        assert_eq!(status.hosts()[0].report(), None);
    });
}

#[cfg(test)]
fn reason_of(status: &ControllerStatus) -> String {
    match status.hosts()[0].confirmation() {
        Confirmation::Failing(reason) => reason.clone(),
        other => panic!("not failing: {other:?}"),
    }
}

#[test]
fn a_refusing_agent_is_reported_by_status_without_its_address() {
    runtime().block_on(async {
        let world = world_with("refusing", NEVER_ANSWERS).await;
        world.commit(&GOOD).await;
        world.hub.tick(&world.poller()).await;
        let reason = reason_of(&world.failing("h").await);
        assert_eq!(reason, "the agent answered 500 Internal Server Error");
    });
}

#[test]
fn an_unreachable_agent_is_reported_by_status_without_its_address() {
    runtime().block_on(async {
        let nowhere = BaseUrl::new("http://127.0.0.1:1");
        let world = world_over("unreachable", Arc::default(), nowhere).await;
        world.commit(&GOOD).await;
        world.hub.tick(&world.poller()).await;
        let reason = reason_of(&world.failing("h").await);
        assert_eq!(reason, "the agent is unreachable");
        assert!(!reason.contains("127.0.0.1"));
    });
}

/// Resolves every peer to one fixed tag, or to none.
#[cfg(test)]
struct Peer(Option<&'static str>);

#[cfg(test)]
impl WhoIs for Peer {
    fn identify(
        &self,
        _peer: &PeerAddress,
    ) -> impl Future<Output = Result<PeerIdentity, WhoIsError>> + Send {
        let tags = self.0.map(|name| TailnetTag::new(name).unwrap());
        ready(Ok(PeerIdentity::new(tags.into_iter().collect())))
    }
}

/// An enrolment desk that always fails, so a request is known to have reached it.
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

/// A controller serving the status and enrolment routes behind the guard, for `caller`.
#[cfg(test)]
async fn guarded(caller: Option<&'static str>) -> String {
    let hub = Hub::new(&[]).unwrap();
    let routes = hub.routes().merge(enroll_routes(Closed));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(serve_controller(listener, Peer(caller), routes));
    url
}

/// The HTTP status the controller answered to a status read and to an enrolment request.
#[cfg(test)]
async fn statuses_of(url: &str) -> (Option<StatusCode>, Option<StatusCode>) {
    let client = ApiClient::new(BaseUrl::new(url)).unwrap();
    let request = EnrollRequest::new("x", vec!["tag:app".to_owned()], SidecarVolume::Empty);
    let status = client.controller_status().await.err();
    let enroll = client.enroll(&request).await.err();
    (refused(status.as_ref()), refused(enroll.as_ref()))
}

#[cfg(test)]
const fn refused(error: Option<&ClientError>) -> Option<StatusCode> {
    match error {
        Some(ClientError::Rejected(status)) => Some(*status),
        _ => None,
    }
}

#[test]
fn one_router_serves_status_and_enrolment_to_agents_only() {
    runtime().block_on(async {
        let (status, enroll) = statuses_of(&guarded(Some("tag:aegis-agent")).await).await;
        assert_eq!(status, None);
        assert_eq!(enroll, Some(StatusCode::BAD_GATEWAY));
        let (status, enroll) = statuses_of(&guarded(None).await).await;
        assert_eq!(status, Some(StatusCode::FORBIDDEN));
        assert_eq!(enroll, Some(StatusCode::FORBIDDEN));
    });
}
