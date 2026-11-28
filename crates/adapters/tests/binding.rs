//! The controller binds what an agent asks to the host it serves and to the desired state: its
//! status reports, its status reads, its key requests and its node deletions, and it bounds what
//! it reads from an agent.

#[cfg(test)]
#[path = "world/mod.rs"]
pub mod world;

use std::future::{Future, ready};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use aegis_adapters::BaseUrl;
use aegis_adapters::api::{ApiClient, ClientError, enroll_routes, node_routes};
use aegis_adapters::tailscale::{ClientId, ClientSecret, OAuthCredentials, TailscaleApi};
use aegis_api::version_1::{
    ControllerStatus, MAX_REPLY_BYTES, ReconcileRequest, RefusalReason, STATUS_PATH, StatusReport,
};
use aegis_application::{
    Allowance, AuthKey, Caller, EnrollError, Enrolled, EnrollmentRequest, Grant, MintLimits,
    SidecarEnroller,
};
use aegis_domain::{HostName, PeerIdentity, TailnetName, TailnetTag, TailnetTags};
use axum::{Extension, Router};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

#[cfg(test)]
const OTHER_HOST: [(&str, &str); 1] = [(
    "hosts/g/web/compose.yaml",
    "services:\n  web:\n    image: nginx:1.27\n",
)];

#[cfg(test)]
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

#[cfg(test)]
const OLD: [(&str, &str); 2] = [
    (
        "hosts/h/old/compose.yaml",
        "services:\n  vault:\n    image: nginx:1.27\n",
    ),
    (
        "hosts/h/old/aegis.toml",
        "state = \"purged\"\ntailnet_tags = [\"tag:app\"]\n\n[expose]\nservice = \"vault\"\nport = 80\n",
    ),
];

#[cfg(test)]
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[cfg(test)]
fn agent_of(node: &str) -> Caller {
    let tag = TailnetTag::new("tag:aegis-agent").unwrap();
    let identity = PeerIdentity::new(TailnetTags::new(vec![tag]));
    Caller::Agent(identity.with_node(HostName::new(node).unwrap()))
}

/// An enrolment desk that mints a key and counts how often it did.
#[cfg(test)]
struct Minting(Arc<AtomicUsize>);

#[cfg(test)]
impl SidecarEnroller for Minting {
    fn enroll(
        &self,
        _request: &EnrollmentRequest,
    ) -> impl Future<Output = Result<Grant, EnrollError>> + Send {
        self.0.fetch_add(1, Ordering::SeqCst);
        let tailnet = TailnetName::new("taildfab0.ts.net").unwrap();
        let key = Enrolled::Minted(AuthKey::new("tskey-minted"));
        ready(Ok(Grant::new(tailnet, key)))
    }
}

/// A controller with the desired state of two hosts, whose agent `h` is called by `caller`s: an
/// agent as `h`, an agent as a node the controller has no agent for, and an operator.
#[cfg(test)]
struct World {
    agent: String,
    bootstrapped: String,
    stranger: String,
    operator: String,
    minted: Arc<AtomicUsize>,
}

#[cfg(test)]
async fn serve_as(routes: &Router, caller: Caller) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let served = routes.clone().layer(Extension(caller));
    tokio::spawn(async move { axum::serve(listener, served).await });
    url
}

#[cfg(test)]
fn allowance(count: u32) -> Allowance {
    Allowance::new(NonZeroU32::try_from(count).unwrap())
}

#[cfg(test)]
async fn world_within(name: &str, limits: MintLimits) -> World {
    let files = [VAULT.to_vec(), OLD.to_vec(), OTHER_HOST.to_vec()].concat();
    let hub = world::loaded_hub(name, &files, limits).await;
    let minted = Arc::new(AtomicUsize::new(0));
    let credentials = OAuthCredentials::new(ClientId::new("id"), ClientSecret::new("secret"));
    let dead = BaseUrl::new("http://127.0.0.1:1");
    let tags = TailnetTags::new(vec![TailnetTag::new("tag:app").unwrap()]);
    let api = TailscaleApi::with_base(dead, credentials)
        .unwrap()
        .for_stack_nodes(tags);
    let desk = enroll_routes(Minting(minted.clone()), hub.binding());
    let routes = hub
        .routes()
        .merge(desk)
        .merge(node_routes(api, hub.binding()));
    World {
        agent: serve_as(&routes, agent_of("h")).await,
        bootstrapped: serve_as(&routes, agent_of("h-aegis-agent")).await,
        stranger: serve_as(&routes, agent_of("stranger")).await,
        operator: serve_as(&routes, Caller::Operator).await,
        minted,
    }
}

#[cfg(test)]
async fn world(name: &str) -> World {
    world_within(name, MintLimits::standard()).await
}

#[cfg(test)]
fn host_names(status: &ControllerStatus) -> Vec<String> {
    let hosts = status.hosts().iter();
    hosts.map(|found| found.host().to_owned()).collect()
}

/// How the controller answers each of `bodies`, asked in turn.
#[cfg(test)]
async fn enroll_each(url: &str, bodies: &[Value]) -> Vec<StatusCode> {
    let mut answers = Vec::new();
    for body in bodies {
        answers.push(enroll_at(url, body).await.status());
    }
    answers
}

#[cfg(test)]
async fn delete_node(url: &str, name: &str) -> reqwest::Response {
    let call = reqwest::Client::new().delete(format!("{url}/v1/nodes/{name}"));
    call.send().await.unwrap()
}

#[cfg(test)]
async fn serve_app(app: Router) -> BaseUrl {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = BaseUrl::new(format!("http://{}", listener.local_addr().unwrap()));
    tokio::spawn(async move { axum::serve(listener, app).await });
    base
}

#[cfg(test)]
async fn status_at(url: &str) -> ControllerStatus {
    let client = ApiClient::new(BaseUrl::new(url)).unwrap();
    client.controller_status().await.unwrap()
}

#[cfg(test)]
async fn post_json(url: &str, path: &str, body: &Value) -> reqwest::Response {
    let call = reqwest::Client::new().post(format!("{url}{path}"));
    call.json(body).send().await.unwrap()
}

#[cfg(test)]
fn wish(stack: &str, hostname: &str, tags: &[&str]) -> Value {
    json!({ "stack": stack, "hostname": hostname, "tags": tags, "state": "empty" })
}

/// `wish` of a node that already has its state.
#[cfg(test)]
fn populated(wish: &Value) -> Value {
    json!({ "stack": wish["stack"], "hostname": wish["hostname"], "tags": wish["tags"], "state": "populated" })
}

#[cfg(test)]
async fn enroll_at(url: &str, body: &Value) -> reqwest::Response {
    post_json(url, "/v1/enroll", body).await
}

#[cfg(test)]
fn vaultwarden() -> Value {
    wish("vault", "vaultwarden", &["tag:app"])
}

#[test]
fn an_agent_cannot_post_a_status_report_for_another_host() {
    runtime().block_on(async {
        let world = world("report").await;
        let own = json!({ "host": "h", "commit": "abc1234", "stacks": [] });
        let other = json!({ "host": "g", "commit": "abc1234", "stacks": [] });
        let own_answer = post_json(&world.agent, STATUS_PATH, &own).await;
        let other_answer = post_json(&world.agent, STATUS_PATH, &other).await;
        assert_eq!(own_answer.status(), StatusCode::NO_CONTENT);
        assert_eq!(other_answer.status(), StatusCode::FORBIDDEN);
        let seen = status_at(&world.operator).await;
        let host = |name: &str| seen.hosts().iter().find(|found| found.host() == name);
        assert!(host("g").unwrap().report().is_none());
    });
}

#[test]
fn a_caller_that_is_no_mapped_agent_gets_nothing_from_the_host_bound_routes() {
    runtime().block_on(async {
        let report = json!({ "host": "stranger", "commit": "abc1234", "stacks": [] });
        let world = world("stranger").await;
        let posted = post_json(&world.stranger, STATUS_PATH, &report).await;
        let read = reqwest::get(format!("{}{STATUS_PATH}", world.stranger)).await;
        let enrolled = enroll_at(&world.stranger, &vaultwarden()).await;
        let deleted = delete_node(&world.stranger, "vault").await;
        let answers = [
            posted.status(),
            read.unwrap().status(),
            enrolled.status(),
            deleted.status(),
        ];
        assert_eq!(answers, [StatusCode::FORBIDDEN; 4]);
        assert_eq!(world.minted.load(Ordering::SeqCst), 0);
    });
}

#[test]
fn an_agent_reads_its_own_host_and_an_operator_reads_the_fleet() {
    runtime().block_on(async {
        let world = world("scope").await;
        assert_eq!(host_names(&status_at(&world.agent).await), ["h"]);
        assert_eq!(host_names(&status_at(&world.operator).await), ["g", "h"]);
    });
}

#[test]
fn the_bootstrap_node_of_an_agent_is_the_agent_of_its_host() {
    runtime().block_on(async {
        let world = world("bootstrapped").await;
        let seen = status_at(&world.bootstrapped).await;
        assert_eq!(host_names(&seen), ["h"]);
        let answer = enroll_at(&world.bootstrapped, &vaultwarden()).await;
        assert_eq!(answer.status(), StatusCode::OK);
    });
}

#[test]
fn a_key_is_minted_for_a_stack_the_desired_state_places_on_the_callers_host() {
    runtime().block_on(async {
        let world = world("placed").await;
        let answer = enroll_at(&world.agent, &vaultwarden()).await;
        assert_eq!(answer.status(), StatusCode::OK);
        assert_eq!(world.minted.load(Ordering::SeqCst), 1);
    });
}

#[test]
fn an_agent_cannot_mint_a_key_for_a_stack_or_hostname_its_host_does_not_have() {
    runtime().block_on(async {
        let world = world("not-placed").await;
        let unknown_stack = wish("nothing", "vaultwarden", &["tag:app"]);
        let other_hostname = wish("vault", "somebody-else", &["tag:app"]);
        let purged_stack = wish("old", "vault", &["tag:app"]);
        let other_hosts_stack = wish("web", "web", &["tag:app"]);
        let bodies = [
            unknown_stack,
            other_hostname,
            purged_stack,
            other_hosts_stack,
        ];
        let answers = enroll_each(&world.agent, &bodies).await;
        assert_eq!(answers, [StatusCode::FORBIDDEN; 4]);
        assert_eq!(world.minted.load(Ordering::SeqCst), 0);
    });
}

#[test]
fn an_agent_cannot_mint_a_key_with_other_tags_than_its_stack_declares() {
    runtime().block_on(async {
        let world = world("tags").await;
        let extra = wish("vault", "vaultwarden", &["tag:app", "tag:admin"]);
        let missing = wish("vault", "vaultwarden", &[]);
        assert_eq!(
            enroll_at(&world.agent, &extra).await.status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            enroll_at(&world.agent, &missing).await.status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(world.minted.load(Ordering::SeqCst), 0);
    });
}

#[test]
fn refusals_are_listed_in_the_status_for_the_host_and_for_operators() {
    runtime().block_on(async {
        let world = world("refusals").await;
        let wrong = wish("vault", "vaultwarden", &["tag:admin"]);
        enroll_at(&world.agent, &wrong).await;
        enroll_at(&world.agent, &wrong).await;
        let seen = status_at(&world.operator).await;
        let refused = &seen.refusals()[0];
        assert_eq!(refused.host(), "h");
        assert_eq!(refused.stack(), Some("vault"));
        assert_eq!(refused.reason(), RefusalReason::WrongTags);
        assert_eq!(refused.times().count(), 2);
        assert_eq!(status_at(&world.agent).await.refusals(), seen.refusals());
    });
}

#[test]
fn repeated_mints_beyond_the_limit_are_refused_and_visible_in_status() {
    runtime().block_on(async {
        let limits = MintLimits::new(allowance(2), allowance(5), allowance(10));
        let world = world_within("limit", limits).await;
        let statuses = [
            enroll_at(&world.agent, &vaultwarden()).await.status(),
            enroll_at(&world.agent, &vaultwarden()).await.status(),
        ];
        assert_eq!(statuses, [StatusCode::OK; 2]);
        let refused = enroll_at(&world.agent, &vaultwarden()).await;
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
        let wait = refused.headers().get("retry-after").unwrap();
        assert!(wait.to_str().unwrap().parse::<u64>().unwrap() > 0);
        assert_eq!(world.minted.load(Ordering::SeqCst), 2);
        let seen = status_at(&world.operator).await;
        assert_eq!(seen.refusals()[0].reason(), RefusalReason::RateLimited);
        assert_eq!(seen.refusals()[0].times().count(), 1);
    });
}

#[test]
fn a_node_that_already_has_state_does_not_count_against_the_limit() {
    runtime().block_on(async {
        let limits = MintLimits::new(allowance(1), allowance(1), allowance(1));
        let world = world_within("populated", limits).await;
        let bodies = [populated(&vaultwarden()), populated(&vaultwarden())];
        let answers = enroll_each(&world.agent, &bodies).await;
        assert_eq!(answers, [StatusCode::OK; 2]);
        assert_eq!(
            enroll_at(&world.agent, &vaultwarden()).await.status(),
            StatusCode::OK
        );
    });
}

#[test]
fn only_the_node_of_a_purged_stack_of_the_callers_host_may_be_deleted() {
    runtime().block_on(async {
        let world = world("purge-check").await;
        let active = delete_node(&world.agent, "vaultwarden").await;
        let unknown = delete_node(&world.agent, "nothing").await;
        assert_eq!(active.status(), StatusCode::FORBIDDEN);
        assert_eq!(unknown.status(), StatusCode::FORBIDDEN);
        let seen = status_at(&world.operator).await;
        assert_eq!(seen.refusals()[0].reason(), RefusalReason::NotPurged);
        let purged = delete_node(&world.agent, "vault").await;
        assert_eq!(
            purged.status(),
            StatusCode::BAD_GATEWAY,
            "it reached Tailscale"
        );
    });
}

#[test]
fn a_status_report_beyond_the_cap_is_refused() {
    runtime().block_on(async {
        let world = world("oversized-report").await;
        let filler = "x".repeat(MAX_REPLY_BYTES);
        let big = json!({ "host": "h", "commit": "abc1234", "stacks": [], "padding": filler });
        let answer = post_json(&world.agent, STATUS_PATH, &big).await;
        assert_eq!(answer.status(), StatusCode::PAYLOAD_TOO_LARGE);
    });
}

/// Write `chunk` to `socket` until the other side hangs up.
#[cfg(test)]
async fn flood(mut socket: tokio::net::TcpStream, chunk: String) {
    while socket.write_all(chunk.as_bytes()).await.is_ok() {}
}

/// Accept one call, write `head` and then, for ever, chunks of `piece` bytes, so a client that
/// reads the whole answer never finishes.
#[cfg(test)]
async fn answer_endlessly(listener: TcpListener, head: &'static str, piece: usize) {
    let (mut socket, _) = listener.accept().await.unwrap();
    let mut request = vec![0_u8; 4096];
    tokio::io::AsyncReadExt::read(&mut socket, &mut request)
        .await
        .unwrap();
    socket.write_all(head.as_bytes()).await.unwrap();
    flood(socket, format!("{piece:x}\r\n{}\r\n", "x".repeat(piece))).await;
}

/// An agent that answers `POST /v1/reconcile` with `head` and then chunks of `piece` bytes.
#[cfg(test)]
async fn endless_agent(head: &'static str, piece: usize) -> BaseUrl {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = BaseUrl::new(format!("http://{}", listener.local_addr().unwrap()));
    tokio::spawn(answer_endlessly(listener, head, piece));
    base
}

#[cfg(test)]
const CHUNKED: &str =
    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ntransfer-encoding: chunked\r\n\r\n";

#[cfg(test)]
const ANNOUNCED: &str =
    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 99999999\r\n\r\n{";

#[test]
fn an_endless_chunked_agent_answer_is_abandoned_at_the_cap() {
    runtime().block_on(async {
        let base = endless_agent(CHUNKED, 65_536).await;
        let client = ApiClient::new(base).unwrap();
        let answer = client.reconcile(&ReconcileRequest::new("abc1234")).await;
        assert!(matches!(answer, Err(ClientError::TooLarge)));
    });
}

#[test]
fn an_agent_answer_that_announces_more_than_the_cap_is_refused_unread() {
    runtime().block_on(async {
        let base = endless_agent(ANNOUNCED, 1).await;
        let client = ApiClient::new(base).unwrap();
        let answer = client.reconcile(&ReconcileRequest::new("abc1234")).await;
        assert!(matches!(answer, Err(ClientError::TooLarge)));
    });
}

#[test]
fn an_agent_answer_within_the_cap_is_read() {
    runtime().block_on(async {
        let report = StatusReport::new("h", "abc1234", Vec::new());
        let kept = report.clone();
        let answer = axum::routing::post(move || ready(axum::response::Json(kept.clone())));
        let base = serve_app(Router::new().route("/v1/reconcile", answer)).await;
        let client = ApiClient::new(base).unwrap();
        let read = client.reconcile(&ReconcileRequest::new("abc1234")).await;
        assert_eq!(read.unwrap(), report);
    });
}
