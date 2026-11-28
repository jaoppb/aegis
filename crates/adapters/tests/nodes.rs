//! `DELETE /v1/nodes/{hostname}`: an agent asks the controller to remove the node of a purged
//! stack, over real sockets, with the controller behind its tag check and a fake Tailscale API.

use std::future::{Future, ready};
use std::sync::{Arc, Mutex};

use aegis_adapters::BaseUrl;
use aegis_adapters::api::{ApiClient, ControllerDesk, node_routes, serve_controller};
use aegis_adapters::tailscale::{ClientId, ClientSecret, OAuthCredentials, TailscaleApi};
use aegis_application::{NodeDeleter, PeerAddress, WhoIs, WhoIsError};
use aegis_domain::{NodeLabel, PeerIdentity, TailnetTag};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use tokio::net::TcpListener;

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

#[cfg(test)]
type Log = Arc<Mutex<Vec<String>>>;

#[cfg(test)]
#[derive(Clone)]
struct Upstream {
    log: Log,
    devices: Vec<Value>,
}

#[cfg(test)]
async fn serve(app: Router) -> BaseUrl {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = BaseUrl::new(format!("http://{}", listener.local_addr().unwrap()));
    tokio::spawn(async move { axum::serve(listener, app).await });
    base
}

#[cfg(test)]
async fn token() -> Json<Value> {
    Json(json!({ "access_token": "t" }))
}

#[cfg(test)]
async fn devices(State(upstream): State<Upstream>) -> Json<Value> {
    Json(json!({ "devices": upstream.devices }))
}

#[cfg(test)]
async fn remove_device(
    State(upstream): State<Upstream>,
    Path(identifier): Path<String>,
) -> StatusCode {
    upstream
        .log
        .lock()
        .unwrap()
        .push(format!("delete {identifier}"));
    StatusCode::OK
}

#[cfg(test)]
fn device(identifier: &str, hostname: &str) -> Value {
    json!({ "id": identifier, "hostname": hostname, "name": format!("{hostname}.taildfab0.ts.net"), "tags": ["tag:app"] })
}

#[cfg(test)]
struct Served {
    base: BaseUrl,
    root: String,
    log: Log,
}

#[cfg(test)]
async fn controller(peer: Option<&'static str>, devices_found: Vec<Value>) -> Served {
    let log = Log::default();
    let upstream = Upstream {
        log: log.clone(),
        devices: devices_found,
    };
    let app = Router::new()
        .route("/api/v2/oauth/token", post(token))
        .route("/api/v2/tailnet/-/devices", get(devices))
        .route("/api/v2/device/{id}", delete(remove_device))
        .with_state(upstream);
    let credentials = OAuthCredentials::new(ClientId::new("id"), ClientSecret::new("secret"));
    let api = TailscaleApi::with_base(serve(app).await, credentials).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let root = format!("http://{}", listener.local_addr().unwrap());
    let base = BaseUrl::new(root.clone());
    tokio::spawn(serve_controller(listener, Peer(peer), node_routes(api)));
    Served { base, root, log }
}

#[cfg(test)]
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[cfg(test)]
fn node(name: &str) -> NodeLabel {
    NodeLabel::new(name).unwrap()
}

#[test]
fn the_node_of_a_purged_stack_is_deleted_through_the_controller() {
    runtime().block_on(async {
        let served = controller(Some("tag:aegis-agent"), vec![device("42", "vault")]).await;
        let desk = ControllerDesk::new(ApiClient::new(served.base).unwrap());
        desk.delete_node(&node("vault")).await.unwrap();
        assert_eq!(*served.log.lock().unwrap(), ["delete 42"]);
    });
}

#[test]
fn a_node_that_is_already_gone_counts_as_deleted() {
    runtime().block_on(async {
        let served = controller(Some("tag:aegis-agent"), vec![device("42", "other")]).await;
        let desk = ControllerDesk::new(ApiClient::new(served.base).unwrap());
        desk.delete_node(&node("vault")).await.unwrap();
        assert!(served.log.lock().unwrap().is_empty());
    });
}

#[test]
fn an_ambiguous_hostname_deletes_nothing_and_fails() {
    runtime().block_on(async {
        let both = vec![device("1", "vault"), device("2", "vault")];
        let served = controller(Some("tag:aegis-agent"), both).await;
        let desk = ControllerDesk::new(ApiClient::new(served.base).unwrap());
        assert!(desk.delete_node(&node("vault")).await.is_err());
        assert!(served.log.lock().unwrap().is_empty());
    });
}

#[test]
fn a_peer_that_is_not_an_agent_is_refused_and_nothing_is_deleted() {
    runtime().block_on(async {
        let served = controller(Some("tag:other"), vec![device("42", "vault")]).await;
        let answer = reqwest::Client::new()
            .delete(format!("{}/v1/nodes/vault", served.root))
            .send()
            .await
            .unwrap();
        assert_eq!(answer.status(), StatusCode::FORBIDDEN);
        assert!(served.log.lock().unwrap().is_empty());
    });
}

#[test]
fn a_hostname_that_is_not_a_dns_label_is_a_bad_request() {
    runtime().block_on(async {
        let served = controller(Some("tag:aegis-agent"), vec![device("42", "vault")]).await;
        let answer = reqwest::Client::new()
            .delete(format!("{}/v1/nodes/Vault_1", served.root))
            .send()
            .await
            .unwrap();
        assert_eq!(answer.status(), StatusCode::BAD_REQUEST);
        assert!(served.log.lock().unwrap().is_empty());
    });
}
