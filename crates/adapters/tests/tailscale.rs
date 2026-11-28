//! The Tailscale adapter against a fake API served on a local socket.

use std::future::{Future, IntoFuture, ready};
use std::sync::{Arc, Mutex};

use aegis_application::{Reason, SidecarState, StateVolume};
use aegis_domain::HostName;

use aegis_adapters::api::BaseUrl;
use aegis_adapters::tailscale::{ClientId, ClientSecret, OAuthCredentials, TailscaleApi};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use tokio::net::TcpListener;

const CLIENT_ID: &str = "client-1";
const CLIENT_SECRET: &str = "tskey-client-supersecret";
const ACCESS_TOKEN: &str = "access-token-xyz";

/// Everything the fake API received, one line per request.
#[derive(Clone, Default)]
struct Fake {
    log: Arc<Mutex<Vec<String>>>,
}

impl Fake {
    fn record(&self, line: &str) {
        self.log
            .lock()
            .into_iter()
            .for_each(|mut log| log.push(line.to_owned()));
    }
}

fn authorized(headers: &HeaderMap) -> bool {
    let expected = format!("Bearer {ACCESS_TOKEN}");
    headers
        .get("authorization")
        .is_some_and(|value| *value == *expected)
}

async fn token(State(fake): State<Fake>, body: String) -> (StatusCode, Json<Value>) {
    fake.record(&format!("POST /api/v2/oauth/token {body}"));
    let valid = body.contains(CLIENT_SECRET) && body.contains("grant_type=client_credentials");
    let reply = json!({ "access_token": ACCESS_TOKEN });
    let status = if valid {
        StatusCode::OK
    } else {
        StatusCode::UNAUTHORIZED
    };
    (status, Json(reply))
}

async fn keys(
    State(fake): State<Fake>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    fake.record(&format!("POST /api/v2/tailnet/-/keys {body}"));
    let status = if authorized(&headers) {
        StatusCode::OK
    } else {
        StatusCode::UNAUTHORIZED
    };
    (status, Json(json!({ "key": "tskey-auth-minted" })))
}

async fn devices(State(fake): State<Fake>, headers: HeaderMap) -> (StatusCode, Json<Value>) {
    fake.record("GET /api/v2/tailnet/-/devices");
    let list = json!({ "devices": [
        { "id": "111", "hostname": "solo" },
        { "id": "222", "hostname": "twin" },
        { "id": "333", "hostname": "Twin" },
        { "id": "444" },
    ]});
    let status = if authorized(&headers) {
        StatusCode::OK
    } else {
        StatusCode::UNAUTHORIZED
    };
    (status, Json(list))
}

async fn remove(State(fake): State<Fake>, Path(device): Path<String>) -> StatusCode {
    fake.record(&format!("DELETE /api/v2/device/{device}"));
    if device == "gone" {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::OK
    }
}

fn router(fake: Fake) -> Router {
    Router::new()
        .route("/api/v2/oauth/token", post(token))
        .route("/api/v2/tailnet/-/keys", post(keys))
        .route("/api/v2/tailnet/-/devices", get(devices))
        .route("/api/v2/device/{device}", delete(remove))
        .with_state(fake)
}

struct Volume(StateVolume);

impl SidecarState for Volume {
    fn volume(&self, _node: &HostName) -> impl Future<Output = Result<StateVolume, Reason>> + Send {
        ready(Ok(self.0))
    }
}

#[cfg(test)]
mod cases {
    use super::*;
    use aegis_application::{
        DeviceId, DeviceRef, EnrollError, Enrolled, Enrollment, KeyMinter, NodeRemover,
        TailnetError,
    };
    use aegis_domain::{TailnetTag, TailnetTags};

    struct Harness {
        api: TailscaleApi,
        fake: Fake,
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn credentials(secret: &str) -> OAuthCredentials {
        OAuthCredentials::new(ClientId::new(CLIENT_ID), ClientSecret::new(secret))
    }

    async fn harness_with(secret: &str) -> Harness {
        let fake = Fake::default();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = BaseUrl::new(format!("http://{}", listener.local_addr().unwrap()));
        tokio::spawn(axum::serve(listener, router(fake.clone())).into_future());
        let api = TailscaleApi::with_base(base, credentials(secret)).unwrap();
        Harness { api, fake }
    }

    async fn harness() -> Harness {
        harness_with(CLIENT_SECRET).await
    }

    fn tags(names: &[&str]) -> TailnetTags {
        names
            .iter()
            .map(|name| TailnetTag::new(*name).unwrap())
            .collect()
    }

    fn lines(fake: &Fake) -> Vec<String> {
        fake.log.lock().unwrap().clone()
    }

    fn create_of(line: &str) -> Value {
        let body: Value = serde_json::from_str(line.split_once("keys ").unwrap().1).unwrap();
        body["capabilities"]["devices"]["create"].clone()
    }

    fn by_hostname(name: &str) -> DeviceRef {
        DeviceRef::Hostname(HostName::new(name).unwrap())
    }

    #[test]
    fn a_key_is_single_use_preauthorized_non_ephemeral_and_tagged() {
        let runtime = runtime();
        let harness = runtime.block_on(harness());
        let key = runtime.block_on(harness.api.mint(&tags(&["tag:app", "tag:web"])));
        assert_eq!(key.unwrap().expose(), "tskey-auth-minted");
        let seen = lines(&harness.fake);
        assert_eq!(seen.len(), 2);
        let create = create_of(&seen[1]);
        assert_eq!(create["reusable"], json!(false));
        assert_eq!(create["ephemeral"], json!(false));
        assert_eq!(create["preauthorized"], json!(true));
        assert_eq!(create["tags"], json!(["tag:app", "tag:web"]));
    }

    #[test]
    fn the_token_is_requested_with_the_client_credentials_grant() {
        let runtime = runtime();
        let harness = runtime.block_on(harness());
        runtime
            .block_on(harness.api.mint(&tags(&["tag:app"])))
            .unwrap();
        let request = lines(&harness.fake)[0].clone();
        assert!(request.contains("client_id=client-1"));
        assert!(request.contains("grant_type=client_credentials"));
    }

    #[test]
    fn a_key_without_tags_is_refused_before_any_call() {
        let runtime = runtime();
        let harness = runtime.block_on(harness());
        let result = runtime.block_on(harness.api.mint(&tags(&[])));
        assert_eq!(result.unwrap_err(), TailnetError::NoTags);
        assert!(lines(&harness.fake).is_empty());
    }

    #[test]
    fn bad_credentials_are_rejected_without_leaking_the_secret() {
        let runtime = runtime();
        let harness = runtime.block_on(harness_with("wrong-secret"));
        let error = runtime
            .block_on(harness.api.mint(&tags(&["tag:app"])))
            .unwrap_err();
        assert!(matches!(error, TailnetError::Rejected(_)));
        assert!(!error.to_string().contains("wrong-secret"));
        assert!(!format!("{:?}", harness.api).contains("wrong-secret"));
    }

    #[test]
    fn a_device_is_deleted_by_id() {
        let runtime = runtime();
        let harness = runtime.block_on(harness());
        let node = DeviceRef::Identifier(DeviceId::new("111").unwrap());
        runtime.block_on(harness.api.remove(&node)).unwrap();
        let seen = lines(&harness.fake);
        assert!(seen[0].starts_with("POST /api/v2/oauth/token"));
        assert_eq!(seen[1], "DELETE /api/v2/device/111");
    }

    #[test]
    fn a_device_is_deleted_by_its_only_hostname_match() {
        let runtime = runtime();
        let harness = runtime.block_on(harness());
        runtime
            .block_on(harness.api.remove(&by_hostname("solo")))
            .unwrap();
        let seen = lines(&harness.fake);
        assert_eq!(seen[1], "GET /api/v2/tailnet/-/devices");
        assert_eq!(seen[2], "DELETE /api/v2/device/111");
    }

    #[test]
    fn an_ambiguous_or_unknown_hostname_deletes_nothing() {
        let runtime = runtime();
        let harness = runtime.block_on(harness());
        let twin = runtime.block_on(harness.api.remove(&by_hostname("twin")));
        let none = runtime.block_on(harness.api.remove(&by_hostname("nobody")));
        assert_eq!(twin, Err(TailnetError::AmbiguousHostname));
        assert_eq!(none, Err(TailnetError::NoSuchDevice));
        assert!(
            lines(&harness.fake)
                .iter()
                .all(|line| !line.starts_with("DELETE"))
        );
    }

    #[test]
    fn a_device_that_is_already_gone_is_reported() {
        let runtime = runtime();
        let harness = runtime.block_on(harness());
        let node = DeviceRef::Identifier(DeviceId::new("gone").unwrap());
        let result = runtime.block_on(harness.api.remove(&node));
        assert_eq!(result, Err(TailnetError::NoSuchDevice));
    }

    fn app() -> HostName {
        HostName::new("app").unwrap()
    }

    fn enroll(
        runtime: &tokio::runtime::Runtime,
        harness: Harness,
        volume: StateVolume,
    ) -> Result<Enrolled, EnrollError> {
        let enrollment = Enrollment::new(Volume(volume), harness.api);
        runtime.block_on(enrollment.enroll(&app(), &tags(&["tag:app"])))
    }

    #[test]
    fn existing_sidecar_state_makes_no_call_to_tailscale() {
        let runtime = runtime();
        let harness = runtime.block_on(harness());
        let fake = harness.fake.clone();
        let result = enroll(&runtime, harness, StateVolume::Populated);
        assert_eq!(result, Ok(Enrolled::AlreadyEnrolled));
        assert!(lines(&fake).is_empty());
    }

    #[test]
    fn an_empty_sidecar_state_mints_a_key_through_the_api() {
        let runtime = runtime();
        let harness = runtime.block_on(harness());
        let fake = harness.fake.clone();
        let result = enroll(&runtime, harness, StateVolume::Empty);
        assert!(matches!(result, Ok(Enrolled::Minted(_))));
        assert_eq!(lines(&fake).len(), 2);
    }
}
