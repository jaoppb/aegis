//! `POST /v1/reconcile` on a real socket: admission, validation of the body, the answer and the
//! report sent to the controller.

use std::future::{Future, ready};
use std::sync::{Arc, Mutex};

use aegis_adapters::api::{ApiClient, BaseUrl, ClientError, reconcile_routes, serve_agent};
use aegis_api::version_1::{ReconcileRequest, STATUS_PATH, StackState, StatusReport};
use aegis_application::{
    GitError, PeerAddress, Reason, Reconcile, ReconcileError, WhoIs, WhoIsError,
};
use aegis_domain::{
    CommitSha, Deployment, HostName, Origin, PeerIdentity, RepoPath, Report, StackReport,
    TailnetTag, TailnetTags,
};
use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use reqwest::StatusCode;
use tokio::net::TcpListener;

#[cfg(test)]
struct Who(Option<&'static str>);

#[cfg(test)]
impl WhoIs for Who {
    fn identify(
        &self,
        _peer: &PeerAddress,
    ) -> impl Future<Output = Result<PeerIdentity, WhoIsError>> + Send {
        let tags = self.0.map(|name| TailnetTag::new(name).unwrap());
        ready(Ok(PeerIdentity::new(
            tags.into_iter().collect::<TailnetTags>(),
        )))
    }
}

#[cfg(test)]
struct Fixed {
    runs: Arc<Mutex<Vec<String>>>,
    failure: Option<&'static str>,
}

#[cfg(test)]
fn success(commit: CommitSha) -> Report {
    let stack = StackReport::new(RepoPath::new("hosts/h/web").unwrap(), Deployment::Running);
    let origin = Origin::new(HostName::new("h").unwrap(), commit);
    Report::new(origin, std::iter::once(stack).collect())
}

#[cfg(test)]
impl Reconcile for Fixed {
    fn reconcile(
        &self,
        commit: CommitSha,
    ) -> impl Future<Output = Result<Report, ReconcileError>> + Send {
        self.runs.lock().unwrap().push(commit.to_string());
        let failed = |text: &str| Err(ReconcileError::Checkout(GitError::new(Reason::new(&text))));
        ready(self.failure.map_or_else(|| Ok(success(commit)), failed))
    }
}

#[cfg(test)]
async fn controller() -> (BaseUrl, Arc<Mutex<Vec<StatusReport>>>) {
    let received = Arc::new(Mutex::new(Vec::new()));
    let handler = |State(seen): State<Arc<Mutex<Vec<StatusReport>>>>, Json(report)| {
        seen.lock().unwrap().push(report);
        ready(StatusCode::ACCEPTED)
    };
    let app = Router::new()
        .route(STATUS_PATH, post(handler))
        .with_state(received.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = BaseUrl::new(format!("http://{}", listener.local_addr().unwrap()));
    tokio::spawn(async move { axum::serve(listener, app).await });
    (base, received)
}

#[cfg(test)]
struct Running {
    client: ApiClient,
    runs: Arc<Mutex<Vec<String>>>,
    reports: Arc<Mutex<Vec<StatusReport>>>,
}

#[cfg(test)]
async fn agent(caller: Option<&'static str>) -> Running {
    let (controller_url, reports) = controller().await;
    let (client, runs) = agent_for(caller, None, controller_url).await;
    Running {
        client,
        runs,
        reports,
    }
}

/// An agent whose reconciler fails with `failure` when set, reporting to `controller_url`.
#[cfg(test)]
async fn agent_for(
    caller: Option<&'static str>,
    failure: Option<&'static str>,
    controller_url: BaseUrl,
) -> (ApiClient, Arc<Mutex<Vec<String>>>) {
    let runs = Arc::new(Mutex::new(Vec::new()));
    let fixed = Fixed {
        runs: runs.clone(),
        failure,
    };
    let routes = reconcile_routes(fixed, ApiClient::new(controller_url).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = BaseUrl::new(format!("http://{}", listener.local_addr().unwrap()));
    tokio::spawn(serve_agent(listener, Who(caller), routes));
    (ApiClient::new(base).unwrap(), runs)
}

#[cfg(test)]
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn the_controller_gets_the_report_and_so_does_the_caller() {
    runtime().block_on(async {
        let running = agent(Some("tag:aegis-controller")).await;
        let answer = running
            .client
            .reconcile(&ReconcileRequest::new("abc1234"))
            .await
            .unwrap();
        assert_eq!(answer.commit(), "abc1234");
        assert_eq!(answer.stacks()[0].state(), StackState::Running);
        assert_eq!(*running.runs.lock().unwrap(), ["abc1234"]);
        assert_eq!(*running.reports.lock().unwrap(), [answer]);
    });
}

#[cfg(test)]
async fn refused(caller: Option<&'static str>) {
    let running = agent(caller).await;
    let result = running
        .client
        .reconcile(&ReconcileRequest::new("abc1234"))
        .await;
    assert!(matches!(
        result,
        Err(ClientError::Rejected(StatusCode::FORBIDDEN))
    ));
    assert!(running.runs.lock().unwrap().is_empty());
}

#[test]
fn an_untagged_or_wrongly_tagged_peer_cannot_start_a_reconcile() {
    runtime().block_on(async {
        refused(None).await;
        refused(Some("tag:aegis-agent")).await;
    });
}

#[cfg(test)]
async fn bad_request(running: &Running, commit: &str) {
    let result = running
        .client
        .reconcile(&ReconcileRequest::new(commit))
        .await;
    assert!(matches!(
        result,
        Err(ClientError::Rejected(StatusCode::BAD_REQUEST))
    ));
}

#[test]
fn a_commit_that_is_not_hex_is_refused_before_anything_runs() {
    runtime().block_on(async {
        let running = agent(Some("tag:aegis-controller")).await;
        bad_request(&running, "--upload-pack=x").await;
        bad_request(&running, "ABCDEF1").await;
        bad_request(&running, "abc").await;
        bad_request(&running, "").await;
        assert!(running.runs.lock().unwrap().is_empty());
    });
}

#[test]
fn a_failed_checkout_is_a_bad_gateway() {
    runtime().block_on(async {
        let (dead, _) = controller().await;
        let (client, _) =
            agent_for(Some("tag:aegis-controller"), Some("no such commit"), dead).await;
        let result = client.reconcile(&ReconcileRequest::new("abc1234")).await;
        assert!(matches!(
            result,
            Err(ClientError::Rejected(StatusCode::BAD_GATEWAY))
        ));
    });
}

#[test]
fn an_unreachable_controller_does_not_fail_the_reconcile() {
    runtime().block_on(async {
        let nowhere = BaseUrl::new("http://127.0.0.1:1");
        let (client, runs) = agent_for(Some("tag:aegis-controller"), None, nowhere).await;
        let answer = client
            .reconcile(&ReconcileRequest::new("abc1234"))
            .await
            .unwrap();
        assert_eq!(answer.commit(), "abc1234");
        assert_eq!(*runs.lock().unwrap(), ["abc1234"]);
    });
}
