//! An agent asks a controller to enrol a sidecar node: both sides on real sockets, the controller
//! behind its tag check and talking to a fake Tailscale API.

use std::future::{Future, ready};
use std::sync::{Arc, Mutex};

#[cfg(test)]
#[path = "world/mod.rs"]
pub mod world;

use aegis_adapters::BaseUrl;
use aegis_adapters::api::{ApiClient, ControllerDesk, enroll_routes, serve_controller};
use aegis_adapters::tailscale::{ClientId, ClientSecret, OAuthCredentials, TailscaleApi};
use aegis_application::{
    EnrollError, Enrolled, Enroller, EnrollmentRequest, MintLimits, PeerAddress, SidecarEnroller,
    SidecarIdentity, StateVolume, WhoIs, WhoIsError,
};
use aegis_domain::{HostName, NodeLabel, PeerIdentity, StackName, TailnetTag, TailnetTags};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use tokio::net::TcpListener;

const AGENT: &str = "tag:aegis-agent";

#[cfg(test)]
/// A stack `vault` of the host `h` that exposes `vaultwarden` with the tag `tag:app`.
const VAULT: [(&str, &str); 2] = [
    (
        "hosts/h/vault/compose.yaml",
        "services:\n  vaultwarden:\n    image: vaultwarden/server:1\n",
    ),
    (
        "hosts/h/vault/aegis.toml",
        "tailnet_tags = [\"tag:app\"]\n\n[expose]\nservice = \"vaultwarden\"\nport = 80\n",
    ),
];

/// Resolves every peer to one fixed tag, or to none, and to the node `h`.
#[cfg(test)]
struct Peer(Option<&'static str>);

#[cfg(test)]
impl WhoIs for Peer {
    fn identify(
        &self,
        _peer: &PeerAddress,
    ) -> impl Future<Output = Result<PeerIdentity, WhoIsError>> + Send {
        let tags = self.0.map(|name| TailnetTag::new(name).unwrap());
        let identity = PeerIdentity::new(tags.into_iter().collect());
        ready(Ok(identity.with_node(HostName::new("h").unwrap())))
    }
}

#[cfg(test)]
type Log = Arc<Mutex<Vec<String>>>;

/// What the fake Tailscale API refuses.
#[cfg(test)]
#[derive(Clone, Copy, PartialEq)]
enum Failing {
    Nothing,
    Devices,
    Keys,
}

#[cfg(test)]
async fn serve(app: Router) -> BaseUrl {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = BaseUrl::new(format!("http://{}", listener.local_addr().unwrap()));
    tokio::spawn(async move { axum::serve(listener, app).await });
    base
}

#[cfg(test)]
#[derive(Clone)]
struct Upstream {
    log: Log,
    failing: Failing,
}

#[cfg(test)]
async fn token(State(upstream): State<Upstream>) -> Json<Value> {
    upstream.log.lock().unwrap().push("token".to_owned());
    Json(json!({ "access_token": "t" }))
}

#[cfg(test)]
async fn keys(
    State(upstream): State<Upstream>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    let tags = &body["capabilities"]["devices"]["create"]["tags"];
    upstream.log.lock().unwrap().push(format!("key {tags}"));
    if upstream.failing == Failing::Keys {
        return StatusCode::FORBIDDEN.into_response();
    }
    Json(json!({ "key": "tskey-auth-minted" })).into_response()
}

#[cfg(test)]
async fn devices(State(upstream): State<Upstream>) -> axum::response::Response {
    if upstream.failing == Failing::Devices {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let device = json!({ "id": "1", "hostname": "ctl", "name": "ctl.taildfab0.ts.net" });
    Json(json!({ "devices": [device] })).into_response()
}

/// A fake Tailscale API that logs what it is asked.
#[cfg(test)]
async fn fake_tailscale(upstream: Upstream) -> BaseUrl {
    let app = Router::new()
        .route("/api/v2/oauth/token", post(token))
        .route("/api/v2/tailnet/-/keys", post(keys))
        .route("/api/v2/tailnet/-/devices", get(devices))
        .with_state(upstream);
    serve(app).await
}

#[cfg(test)]
fn tags(names: &[&str]) -> TailnetTags {
    names
        .iter()
        .map(|name| TailnetTag::new(*name).unwrap())
        .collect()
}

/// A controller whose peers are all tagged `peer`, and the log of the Tailscale API behind it.
#[cfg(test)]
struct Served {
    base: BaseUrl,
    root: String,
    log: Log,
}

#[cfg(test)]
async fn controller(peer: Option<&'static str>, failing: Failing) -> Served {
    let log = Log::default();
    let upstream = Upstream {
        log: log.clone(),
        failing,
    };
    let credentials = OAuthCredentials::new(ClientId::new("id"), ClientSecret::new("secret"));
    let api = TailscaleApi::with_base(fake_tailscale(upstream).await, credentials).unwrap();
    let enroller = Enroller::new(api.clone(), api, tags(&["tag:app"]));
    let hub = world::loaded_hub("enroll", &VAULT, MintLimits::standard()).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let root = format!("http://{}", listener.local_addr().unwrap());
    let base = BaseUrl::new(root.clone());
    tokio::spawn(serve_controller(
        listener,
        Peer(peer),
        enroll_routes(enroller, hub.binding()),
    ));
    Served { base, root, log }
}

#[cfg(test)]
fn request(state: StateVolume, wanted: &[&str]) -> EnrollmentRequest {
    let node = NodeLabel::new("vaultwarden").unwrap();
    let identity = SidecarIdentity::new(StackName::new("vault").unwrap(), node);
    EnrollmentRequest::new(identity, tags(wanted), state)
}

#[cfg(test)]
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[cfg(test)]
fn minted_keys(log: &Log) -> usize {
    let lines = log.lock().unwrap();
    lines.iter().filter(|line| line.starts_with("key")).count()
}

/// The status the controller answers a raw body with.
#[cfg(test)]
async fn post_status(
    peer: Option<&'static str>,
    failing: Failing,
    body: &str,
) -> (StatusCode, Log) {
    let served = controller(peer, failing).await;
    let client = reqwest::Client::new();
    let call = client.post(format!("{}/v1/enroll", served.root));
    let answer = call
        .header("content-type", "application/json")
        .body(body.to_owned())
        .send()
        .await
        .unwrap();
    (answer.status(), served.log)
}

#[cfg(test)]
const VALID: &str =
    r#"{"stack":"vault","hostname":"vaultwarden","tags":["tag:app"],"state":"empty"}"#;

#[test]
fn a_new_node_is_enrolled_with_a_key_and_the_tailnet_name() {
    runtime().block_on(async {
        let served = controller(Some(AGENT), Failing::Nothing).await;
        let log = served.log.clone();
        let desk = ControllerDesk::new(ApiClient::new(served.base).unwrap());
        let asked = request(StateVolume::Empty, &["tag:app"]);
        let grant = desk.enroll(&asked).await.unwrap();
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
        let served = controller(Some(AGENT), Failing::Nothing).await;
        let log = served.log.clone();
        let desk = ControllerDesk::new(ApiClient::new(served.base).unwrap());
        let first = request(StateVolume::Empty, &["tag:app"]);
        desk.enroll(&first).await.unwrap();
        let again = request(StateVolume::Populated, &["tag:app"]);
        let grant = desk.enroll(&again).await.unwrap();
        assert_eq!(grant.enrolled(), &Enrolled::AlreadyEnrolled);
        assert_eq!(grant.tailnet().as_str(), "taildfab0.ts.net");
        assert_eq!(minted_keys(&log), 1);
    });
}

#[test]
fn a_tag_the_controller_does_not_allow_is_refused_and_mints_nothing() {
    runtime().block_on(async {
        let (status, log) = post_status(
            Some(AGENT),
            Failing::Nothing,
            r#"{"stack":"vault","hostname":"vaultwarden","tags":["tag:admin"],"state":"empty"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(minted_keys(&log), 0);
    });
}

#[test]
fn a_bad_hostname_tag_or_stack_name_is_a_bad_request() {
    runtime().block_on(async {
        let bad_host =
            r#"{"stack":"vault","hostname":"Not_A_Host","tags":["tag:app"],"state":"empty"}"#;
        let (host, _) = post_status(Some(AGENT), Failing::Nothing, bad_host).await;
        let bad_tag = r#"{"stack":"vault","hostname":"v","tags":["app"],"state":"empty"}"#;
        let (tag, _) = post_status(Some(AGENT), Failing::Nothing, bad_tag).await;
        let bad_stack = r#"{"stack":"Not A Stack","hostname":"v","tags":[],"state":"empty"}"#;
        let (stack, _) = post_status(Some(AGENT), Failing::Nothing, bad_stack).await;
        assert_eq!([host, tag, stack], [StatusCode::BAD_REQUEST; 3]);
    });
}

#[test]
fn a_tailscale_failure_is_a_bad_gateway() {
    runtime().block_on(async {
        let (devices, _) = post_status(Some(AGENT), Failing::Devices, VALID).await;
        let (keys, log) = post_status(Some(AGENT), Failing::Keys, VALID).await;
        assert_eq!([devices, keys], [StatusCode::BAD_GATEWAY; 2]);
        assert_eq!(minted_keys(&log), 1);
    });
}

#[test]
fn only_an_agent_may_enrol_a_node() {
    runtime().block_on(async {
        let (untagged, log) = post_status(None, Failing::Nothing, VALID).await;
        let (controller_peer, _) =
            post_status(Some("tag:aegis-controller"), Failing::Nothing, VALID).await;
        let (stranger, _) = post_status(Some("tag:other"), Failing::Nothing, VALID).await;
        let (agent, _) = post_status(Some(AGENT), Failing::Nothing, VALID).await;
        assert_eq!(
            [untagged, controller_peer, stranger],
            [StatusCode::FORBIDDEN; 3]
        );
        assert_eq!(agent, StatusCode::OK);
        assert_eq!(minted_keys(&log), 0);
    });
}

#[test]
fn a_controller_that_cannot_be_reached_is_a_controller_error() {
    runtime().block_on(async {
        let base = BaseUrl::new("http://127.0.0.1:1");
        let desk = ControllerDesk::new(ApiClient::new(base).unwrap());
        let result = desk
            .enroll(&request(StateVolume::Empty, &["tag:app"]))
            .await;
        assert!(matches!(result, Err(EnrollError::Controller(_))));
    });
}
