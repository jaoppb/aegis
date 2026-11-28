//! The controller end to end: a real git origin, the polling adapters, the status routes on a real
//! socket and a fake agent that answers `POST /v1/reconcile`.

use std::future::ready;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aegis_adapters::api::{AgentAddress, ApiClient, Hub};
use aegis_adapters::{BaseUrl, FsRepo, GitCheckout, Remote, SerdeStackParser, SopsCheck};
use aegis_api::version_1::{
    Agent, Confirmation, ControllerStatus, RECONCILE_PATH, ReconcileRequest, STATUS_PATH,
    StackState, StackStatus, StatusReport,
};
use aegis_application::{Poller, Repository};
use aegis_domain::HostName;
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

    fn poller(&self) -> Poller<GitCheckout> {
        let work = self.root.join("work");
        let remote = Remote::new(self.origin().to_str().unwrap(), "trunk").unwrap();
        let git = GitCheckout::new(&work).with_remote(remote);
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
    let mut status = client.controller_status().await.unwrap();
    let mut tries: u32 = 0;
    while !has_confirmed(&status, host) && tries < 250 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        status = client.controller_status().await.unwrap();
        tries = tries.saturating_add(1);
    }
    assert!(has_confirmed(&status, host), "{host} never confirmed");
    status
}

#[cfg(test)]
fn has_confirmed(status: &ControllerStatus, host: &str) -> bool {
    let found = status.hosts().iter().find(|found| found.host() == host);
    found.is_some_and(|found| found.confirmation() == Confirmation::Confirmed)
}

#[cfg(test)]
async fn fake_agent(calls: Arc<Mutex<Vec<String>>>) -> BaseUrl {
    let handler = |State(seen): State<Arc<Mutex<Vec<String>>>>,
                   Json(request): Json<ReconcileRequest>| {
        seen.lock().unwrap().push(request.commit().to_owned());
        let stack = StackStatus::new("hosts/h/web", StackState::Running, Vec::new());
        ready(Json(StatusReport::new("h", request.commit(), vec![stack])))
    };
    let app = Router::new()
        .route(RECONCILE_PATH, post(handler))
        .with_state(calls);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = BaseUrl::new(format!("http://{}", listener.local_addr().unwrap()));
    tokio::spawn(async move { axum::serve(listener, app).await });
    base
}

#[cfg(test)]
async fn world(name: &str) -> World {
    let root = std::env::temp_dir().join(format!("aegis-ctl-{}-{name}", std::process::id()));
    tokio::fs::remove_dir_all(&root).await.ok();
    tokio::fs::create_dir_all(root.join("origin"))
        .await
        .unwrap();
    git(&root.join("origin"), &["init", "-q", "-b", "trunk"]).await;
    let calls = Arc::new(Mutex::new(Vec::new()));
    let agent = fake_agent(calls.clone()).await;
    let address = AgentAddress::new(HostName::new("h").unwrap(), agent);
    let hub = Hub::new(&[address]).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let routes = hub.routes();
    tokio::spawn(async move { axum::serve(listener, routes).await });
    World {
        root,
        calls,
        hub,
        url,
    }
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
        assert_eq!(status.hosts()[0].confirmation(), Confirmation::Confirmed);
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
        assert_eq!(lonely.unwrap().agent(), Agent::Unmapped);
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
