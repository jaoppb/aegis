//! An agent asks a controller to enrol a sidecar node: both sides on real sockets, the controller
//! talking to a fake Tailscale API.

use std::future::{Future, ready};
use std::sync::{Arc, Mutex};

use aegis_adapters::BaseUrl;
use aegis_adapters::api::{ApiClient, ControllerDesk, enroll_routes, serve_agent};
use aegis_adapters::tailscale::{ClientId, ClientSecret, OAuthCredentials, TailscaleApi};
use aegis_application::{
    EnrollError, Enrolled, Enroller, EnrollmentRequest, PeerAddress, SidecarEnroller, StateVolume,
    WhoIs, WhoIsError,
};
use aegis_domain::{NodeLabel, PeerIdentity, TailnetTag, TailnetTags};
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use tokio::net::TcpListener;

#[cfg(test)]
struct Agent;

#[cfg(test)]
impl WhoIs for Agent {
    fn identify(
        &self,
        _peer: &PeerAddress,
    ) -> impl Future<Output = Result<PeerIdentity, WhoIsError>> + Send {
        let tag = TailnetTag::new("tag:aegis-controller").unwrap();
        ready(Ok(PeerIdentity::new(std::iter::once(tag).collect())))
    }
}

type Log = Arc<Mutex<Vec<String>>>;

#[cfg(test)]
async fn serve(app: Router) -> BaseUrl {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = BaseUrl::new(format!("http://{}", listener.local_addr().unwrap()));
    tokio::spawn(async move { axum::serve(listener, app).await });
    base
}

/// A fake Tailscale API that counts the keys it mints.
#[cfg(test)]
async fn fake_tailscale(log: Log) -> BaseUrl {
    let token = |State(log): State<Log>| async move {
        log.lock().unwrap().push("token".to_owned());
        Json(json!({ "access_token": "t" }))
    };
    let keys = |State(log): State<Log>, Json(body): Json<Value>| async move {
        log.lock().unwrap().push(format!(
            "key {}",
            body["capabilities"]["devices"]["create"]["tags"]
        ));
        Json(json!({ "key": "tskey-auth-minted" }))
    };
    let devices = || async {
        Json(
            json!({ "devices": [{ "id": "1", "hostname": "ctl", "name": "ctl.taildfab0.ts.net" }] }),
        )
    };
    let app = Router::new()
        .route("/api/v2/oauth/token", post(token))
        .route("/api/v2/tailnet/-/keys", post(keys))
        .route("/api/v2/tailnet/-/devices", get(devices))
        .with_state(log);
    serve(app).await
}

#[cfg(test)]
fn tags(names: &[&str]) -> TailnetTags {
    names
        .iter()
        .map(|name| TailnetTag::new(*name).unwrap())
        .collect()
}

#[cfg(test)]
async fn desk(log: Log) -> ControllerDesk {
    let upstream = fake_tailscale(log).await;
    let credentials = OAuthCredentials::new(ClientId::new("id"), ClientSecret::new("secret"));
    let api = TailscaleApi::with_base(upstream, credentials).unwrap();
    let enroller = Enroller::new(api.clone(), api, tags(&["tag:app"]));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = BaseUrl::new(format!("http://{}", listener.local_addr().unwrap()));
    let routes = enroll_routes(enroller);
    tokio::spawn(serve_agent(listener, Agent, routes));
    ControllerDesk::new(ApiClient::new(base).unwrap())
}

#[cfg(test)]
fn request(state: StateVolume, wanted: &[&str]) -> EnrollmentRequest {
    EnrollmentRequest::new(NodeLabel::new("vaultwarden").unwrap(), tags(wanted), state)
}

#[test]
fn a_new_node_is_enrolled_with_a_key_and_the_tailnet_name() {
    runtime().block_on(async {
        let log = Log::default();
        let desk = desk(log.clone()).await;
        let grant = desk
            .enroll(&request(StateVolume::Empty, &["tag:app"]))
            .await
            .unwrap();
        assert_eq!(grant.tailnet().as_str(), "taildfab0.ts.net");
        assert!(
            matches!(grant.enrolled(), Enrolled::Minted(key) if key.expose() == "tskey-auth-minted")
        );
        assert!(
            log.lock()
                .unwrap()
                .iter()
                .any(|line| line == "key [\"tag:app\"]")
        );
    });
}

#[test]
fn a_redeploy_keeps_the_node_and_mints_no_second_key() {
    runtime().block_on(async {
        let log = Log::default();
        let desk = desk(log.clone()).await;
        desk.enroll(&request(StateVolume::Empty, &["tag:app"]))
            .await
            .unwrap();
        let again = desk
            .enroll(&request(StateVolume::Populated, &["tag:app"]))
            .await
            .unwrap();
        assert_eq!(again.enrolled(), &Enrolled::AlreadyEnrolled);
        assert_eq!(again.tailnet().as_str(), "taildfab0.ts.net");
        let keys = log
            .lock()
            .unwrap()
            .iter()
            .filter(|line| line.starts_with("key"))
            .count();
        assert_eq!(keys, 1);
    });
}

#[test]
fn a_tag_the_controller_does_not_allow_is_refused_and_mints_nothing() {
    runtime().block_on(async {
        let log = Log::default();
        let desk = desk(log.clone()).await;
        let result = desk
            .enroll(&request(StateVolume::Empty, &["tag:admin"]))
            .await;
        assert!(
            matches!(result, Err(EnrollError::Controller(_))),
            "{result:?}"
        );
        assert!(
            log.lock()
                .unwrap()
                .iter()
                .all(|line| !line.starts_with("key"))
        );
    });
}

#[test]
fn a_node_without_a_tag_is_refused() {
    runtime().block_on(async {
        let desk = desk(Log::default()).await;
        let result = desk.enroll(&request(StateVolume::Empty, &[])).await;
        assert!(matches!(result, Err(EnrollError::Controller(_))));
    });
}

#[cfg(test)]
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}
