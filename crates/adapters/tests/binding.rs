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
use aegis_adapters::api::{AgentAddress, ApiClient, ClientError, Hub, enroll_routes, node_routes};
use aegis_adapters::tailscale::{ClientId, ClientSecret, OAuthCredentials, TailscaleApi};
use aegis_api::version_1::{
    ControllerStatus, MAX_REPLY_BYTES, ReconcileRequest, RefusalReason, StatusReport,
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

/// Stacks that fail to load: one under `h`, one under `g` and one under `hh`, whose directory
/// starts like `h`'s.
#[cfg(test)]
const BROKEN: [(&str, &str); 3] = [
    ("hosts/h/broken/readme", "no compose file"),
    ("hosts/g/broken/readme", "no compose file"),
    ("hosts/hh/broken/readme", "no compose file"),
];

/// A stack of `g` that is purged, one of `h` whose node name `g` moved to, and the active stack of
/// `g` that uses it now.
#[cfg(test)]
const MOVED: [(&str, &str); 6] = [
    (
        "hosts/g/gone/compose.yaml",
        "services:\n  gnode:\n    image: nginx:1.27\n",
    ),
    (
        "hosts/g/gone/aegis.toml",
        "state = \"purged\"\ntailnet_tags = [\"tag:app\"]\n\n[expose]\nservice = \"gnode\"\nport = 80\n",
    ),
    (
        "hosts/h/moved/compose.yaml",
        "services:\n  moved:\n    image: nginx:1.27\n",
    ),
    (
        "hosts/h/moved/aegis.toml",
        "state = \"purged\"\ntailnet_tags = [\"tag:app\"]\n\n[expose]\nservice = \"moved\"\nport = 80\n",
    ),
    (
        "hosts/g/moved/compose.yaml",
        "services:\n  moved:\n    image: nginx:1.27\n",
    ),
    (
        "hosts/g/moved/aegis.toml",
        "tailnet_tags = [\"tag:app\"]\n\n[expose]\nservice = \"moved\"\nport = 80\n",
    ),
];

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
    agent_g: String,
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
    let parts = [VAULT.to_vec(), OLD.to_vec(), OTHER_HOST.to_vec()];
    let files = [parts.concat(), BROKEN.to_vec(), MOVED.to_vec()].concat();
    world_over(world::loaded_hub(name, &files, limits).await).await
}

/// A world where every stack loads, so a purged stack may have its node deleted.
#[cfg(test)]
async fn clean_world(name: &str) -> World {
    let parts = [
        VAULT.to_vec(),
        OLD.to_vec(),
        OTHER_HOST.to_vec(),
        MOVED.to_vec(),
    ];
    let hub = world::loaded_hub(name, &parts.concat(), MintLimits::standard()).await;
    world_over(hub).await
}

/// A world over a hub that has not loaded a commit: it only knows the agents of `h` and `g`.
#[cfg(test)]
async fn unloaded_world() -> World {
    let nowhere = |name: &str| {
        let host = HostName::new(name).unwrap();
        AgentAddress::new(host, BaseUrl::new("http://127.0.0.1:1"))
    };
    let hub = Hub::limited(&[nowhere("h"), nowhere("g")], MintLimits::standard());
    world_over(hub.unwrap()).await
}

#[cfg(test)]
async fn world_over(hub: Hub) -> World {
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
        agent_g: serve_as(&routes, agent_of("g")).await,
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
async fn put_json(url: &str, path: &str, body: &Value) -> reqwest::Response {
    let call = reqwest::Client::new().put(format!("{url}{path}"));
    call.json(body).send().await.unwrap()
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
    post_json(url, "/v1/enrollments", body).await
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
        let own_answer = put_json(&world.agent, "/v1/hosts/h/status", &own).await;
        let other_answer = put_json(&world.agent, "/v1/hosts/g/status", &other).await;
        let crossed = put_json(&world.agent, "/v1/hosts/h/status", &other).await;
        assert_eq!(own_answer.status(), StatusCode::NO_CONTENT);
        assert_eq!(other_answer.status(), StatusCode::FORBIDDEN);
        assert_eq!(crossed.status(), StatusCode::BAD_REQUEST);
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
        let posted = put_json(&world.stranger, "/v1/hosts/stranger/status", &report).await;
        let read = reqwest::get(format!("{}/v1/fleet", world.stranger)).await;
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
        assert_eq!(
            host_names(&status_at(&world.operator).await),
            ["g", "h", "hh"]
        );
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
        let world = clean_world("purge-check").await;
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
        let answer = put_json(&world.agent, "/v1/hosts/h/status", &big).await;
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

/// An agent that answers `POST /v1/reconciliations` with `head` and then chunks of `piece` bytes.
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
        let request = ReconcileRequest::new("abc1234");
        let call = client.reconcile(&request);
        let answer = tokio::time::timeout(std::time::Duration::from_secs(10), call).await;
        assert!(matches!(answer, Ok(Err(ClientError::TooLarge))));
    });
}

#[test]
fn an_agent_answer_that_announces_more_than_the_cap_is_refused_unread() {
    runtime().block_on(async {
        let base = endless_agent(ANNOUNCED, 1).await;
        let client = ApiClient::new(base).unwrap();
        let request = ReconcileRequest::new("abc1234");
        let call = client.reconcile(&request);
        let answer = tokio::time::timeout(std::time::Duration::from_secs(10), call).await;
        assert!(matches!(answer, Ok(Err(ClientError::TooLarge))));
    });
}

#[test]
fn an_agent_answer_within_the_cap_is_read() {
    runtime().block_on(async {
        let report = StatusReport::new("h", "abc1234", Vec::new());
        let kept = report.clone();
        let answer = axum::routing::post(move || ready(axum::response::Json(kept.clone())));
        let base = serve_app(Router::new().route("/v1/reconciliations", answer)).await;
        let client = ApiClient::new(base).unwrap();
        let read = client.reconcile(&ReconcileRequest::new("abc1234")).await;
        assert_eq!(read.unwrap(), report);
    });
}

#[cfg(test)]
fn refusing_hosts(status: &ControllerStatus) -> Vec<String> {
    let refusals = status.refusals().iter();
    refusals.map(|each| each.host().to_owned()).collect()
}

#[cfg(test)]
fn rejected_directories(status: &ControllerStatus) -> Vec<String> {
    let rejected = status.target().unwrap().rejected().iter();
    rejected.map(|each| each.directory().to_owned()).collect()
}

#[test]
fn an_agent_sees_only_its_own_refusals_and_an_operator_sees_all() {
    runtime().block_on(async {
        let world = world("own-refusals").await;
        let wrong = wish("vault", "vaultwarden", &["tag:admin"]);
        enroll_at(&world.agent, &wrong).await;
        enroll_at(&world.agent_g, &vaultwarden()).await;
        assert_eq!(refusing_hosts(&status_at(&world.agent).await), ["h"]);
        assert_eq!(refusing_hosts(&status_at(&world.agent_g).await), ["g"]);
        assert_eq!(
            refusing_hosts(&status_at(&world.operator).await),
            ["h", "g"]
        );
    });
}

#[test]
fn a_made_up_stack_name_is_not_kept_in_the_status() {
    runtime().block_on(async {
        let world = world("made-up").await;
        let invented = wish("invented", "vaultwarden", &["tag:app"]);
        assert_eq!(
            enroll_at(&world.agent, &invented).await.status(),
            StatusCode::FORBIDDEN
        );
        let seen = status_at(&world.operator).await;
        assert_eq!(seen.refusals()[0].reason(), RefusalReason::NotPlaced);
        assert_eq!(seen.refusals()[0].stack(), None);
    });
}

#[test]
fn a_node_of_a_purged_stack_of_another_host_or_still_in_use_is_not_deleted() {
    runtime().block_on(async {
        let world = clean_world("not-mine").await;
        let theirs = delete_node(&world.agent, "gnode").await;
        let moved = delete_node(&world.agent, "moved").await;
        assert_eq!(theirs.status(), StatusCode::FORBIDDEN);
        assert_eq!(moved.status(), StatusCode::FORBIDDEN);
        let seen = status_at(&world.operator).await;
        assert!(
            seen.refusals()
                .iter()
                .all(|each| each.reason() == RefusalReason::NotPurged)
        );
        let own = delete_node(&world.agent_g, "gnode").await;
        assert_eq!(
            own.status(),
            StatusCode::BAD_GATEWAY,
            "g's own purged node reaches Tailscale"
        );
    });
}

#[test]
fn no_node_is_deleted_while_a_stack_of_the_repository_is_rejected() {
    runtime().block_on(async {
        let world = world("rejected-blocks").await;
        let own = delete_node(&world.agent, "vault").await;
        assert_eq!(own.status(), StatusCode::FORBIDDEN);
    });
}

#[test]
fn an_operator_cannot_report_enrol_or_delete_through_the_agent_routes() {
    runtime().block_on(async {
        let world = world("operator").await;
        let report = json!({ "host": "h", "commit": "abc1234", "stacks": [] });
        let posted = put_json(&world.operator, "/v1/hosts/h/status", &report).await;
        let enrolled = enroll_at(&world.operator, &vaultwarden()).await;
        let deleted = delete_node(&world.operator, "vault").await;
        let answers = [posted.status(), enrolled.status(), deleted.status()];
        assert_eq!(answers, [StatusCode::FORBIDDEN; 3]);
        assert_eq!(world.minted.load(Ordering::SeqCst), 0);
    });
}

#[test]
fn before_a_commit_loads_enrolment_and_deletion_wait_and_the_agent_sees_no_target() {
    runtime().block_on(async {
        let world = unloaded_world().await;
        let enrolled = enroll_at(&world.agent, &vaultwarden()).await;
        let deleted = delete_node(&world.agent, "vault").await;
        assert_eq!(enrolled.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(deleted.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(world.minted.load(Ordering::SeqCst), 0);
        assert!(status_at(&world.agent).await.target().is_none());
        assert!(status_at(&world.operator).await.refusals().is_empty());
    });
}

#[test]
fn an_agent_sees_only_the_rejected_stacks_of_its_own_host() {
    runtime().block_on(async {
        let world = world("rejected").await;
        assert_eq!(
            rejected_directories(&status_at(&world.agent).await),
            ["hosts/h/broken"]
        );
        assert_eq!(
            rejected_directories(&status_at(&world.operator).await).len(),
            3
        );
    });
}

#[cfg(test)]
async fn old_status(url: &str, path: &str, method: reqwest::Method) -> StatusCode {
    let body = json!({ "host": "h", "commit": "abc1234", "stacks": [] });
    let call = reqwest::Client::new().request(method, format!("{url}{path}"));
    call.json(&body).send().await.unwrap().status()
}

#[test]
fn the_verb_paths_of_the_old_api_are_gone_and_the_new_ones_take_only_their_method() {
    runtime().block_on(async {
        let world = world("renamed").await;
        let post = reqwest::Method::POST;
        let enroll = old_status(&world.agent, "/v1/enroll", post.clone()).await;
        let status = old_status(&world.agent, "/v1/status", post.clone()).await;
        let wrong = old_status(&world.agent, "/v1/hosts/h/status", post).await;
        let missing = StatusCode::NOT_FOUND;
        let answers = [enroll, status, wrong];
        assert_eq!(answers, [missing, missing, StatusCode::METHOD_NOT_ALLOWED]);
        assert_eq!(world.minted.load(Ordering::SeqCst), 0);
    });
}
