//! `POST /v1/reconcile` on a real socket: admission, validation of the body, the answer and the
//! report sent to the controller.

use std::future::{Future, ready};
use std::sync::{Arc, Mutex};

use aegis_adapters::api::{ApiClient, BaseUrl, ClientError, reconcile_routes, serve_agent};
use aegis_api::version_1::{ReconcileRequest, STATUS_PATH, StackState, StatusReport};
use aegis_application::{PeerAddress, Reconcile, ReconcileError, WhoIs, WhoIsError};
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
}

#[cfg(test)]
impl Reconcile for Fixed {
    fn reconcile(
        &self,
        commit: CommitSha,
    ) -> impl Future<Output = Result<Report, ReconcileError>> + Send {
        self.runs.lock().unwrap().push(commit.to_string());
        let stack = StackReport::new(RepoPath::new("hosts/h/web").unwrap(), Deployment::Running);
        let origin = Origin::new(HostName::new("h").unwrap(), commit);
        ready(Ok(Report::new(origin, std::iter::once(stack).collect())))
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
    let runs = Arc::new(Mutex::new(Vec::new()));
    let routes = reconcile_routes(
        Fixed { runs: runs.clone() },
        ApiClient::new(controller_url).unwrap(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = BaseUrl::new(format!("http://{}", listener.local_addr().unwrap()));
    tokio::spawn(serve_agent(listener, Who(caller), routes));
    Running {
        client: ApiClient::new(base).unwrap(),
        runs,
        reports,
    }
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
