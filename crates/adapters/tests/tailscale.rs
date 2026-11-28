//! The Tailscale adapter against a fake API served on a local socket.

use std::future::{Future, ready};

use aegis_application::{Reason, SidecarState, StateVolume};
use aegis_domain::HostName;

struct Volume(StateVolume);

impl SidecarState for Volume {
    fn volume(&self, _node: &HostName) -> impl Future<Output = Result<StateVolume, Reason>> + Send {
        ready(Ok(self.0))
    }
}

#[cfg(test)]
mod cases {
    use std::future::IntoFuture;
    use std::sync::{Arc, Mutex};

    use super::Volume;
    use aegis_adapters::BaseUrl;
    use aegis_adapters::tailscale::{ClientId, ClientSecret, OAuthCredentials, TailscaleApi};
    use aegis_application::{
        DeviceId, DeviceRef, EnrollError, Enrolled, Enrollment, KeyMinter, NodeRemover,
        StateVolume, TailnetError,
    };
    use aegis_domain::{HostName, TailnetTag, TailnetTags};
    use axum::extract::{Path, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::{IntoResponse, Response};
    use axum::routing::{delete, get, post};
    use axum::{Json, Router};
    use serde_json::{Value, json};
    use tokio::net::TcpListener;

    const CLIENT_ID: &str = "client-1";
    const CLIENT_SECRET: &str = "tskey-client-supersecret";
    const ACCESS_TOKEN: &str = "access-token-xyz";
    const MINTED_KEY: &str = "tskey-auth-minted";

    /// How the fake answers.
    #[derive(Clone)]
    enum Behavior {
        Normal,
        /// Every call except the token exchange answers this status.
        Status(StatusCode),
        /// Every answer has the right fields with the wrong types, holding the secret.
        Malformed,
        /// Every answer is larger than the client accepts.
        Oversized,
        /// The token exchange redirects to this URL.
        Redirect(String),
    }

    /// Everything the fake received, one line per request.
    #[derive(Clone)]
    struct Fake {
        log: Arc<Mutex<Vec<String>>>,
        behavior: Behavior,
    }

    struct Served {
        root: String,
        fake: Fake,
    }

    struct Harness {
        api: TailscaleApi,
        fake: Fake,
    }

    fn record(fake: &Fake, line: &str) {
        fake.log.lock().unwrap().push(line.to_owned());
    }

    fn lines(fake: &Fake) -> Vec<String> {
        fake.log.lock().unwrap().clone()
    }

    fn authorized(headers: &HeaderMap) -> bool {
        let expected = format!("Bearer {ACCESS_TOKEN}");
        headers
            .get("authorization")
            .is_some_and(|value| *value == *expected)
    }

    /// The answer to an authorized call: `answer`, or what the behavior makes of it.
    fn reply(fake: &Fake, headers: &HeaderMap, answer: Value) -> Response {
        let wrong = json!({ "key": [CLIENT_SECRET], "devices": [CLIENT_SECRET] });
        match (&fake.behavior, authorized(headers)) {
            (_, false) => StatusCode::UNAUTHORIZED.into_response(),
            (Behavior::Status(code), _) => (*code, Json(json!({}))).into_response(),
            (Behavior::Malformed, _) => Json(wrong).into_response(),
            (Behavior::Oversized, _) => Json(json!({ "key": "k".repeat(2 << 20) })).into_response(),
            _ => Json(answer).into_response(),
        }
    }

    fn redirect(url: &str) -> Response {
        (StatusCode::TEMPORARY_REDIRECT, [("location", url)]).into_response()
    }

    async fn token(State(fake): State<Fake>, body: String) -> Response {
        record(&fake, &format!("POST /api/v2/oauth/token {body}"));
        let valid = body.contains(CLIENT_SECRET) && body.contains("grant_type=client_credentials");
        let granted = json!({ "access_token": ACCESS_TOKEN });
        let wrong = json!({ "access_token": [CLIENT_SECRET] });
        match (&fake.behavior, valid) {
            (Behavior::Redirect(url), _) => redirect(url),
            (_, false) => StatusCode::UNAUTHORIZED.into_response(),
            (Behavior::Malformed, _) => Json(wrong).into_response(),
            _ => Json(granted).into_response(),
        }
    }

    async fn keys(
        State(fake): State<Fake>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Response {
        record(&fake, &format!("POST /api/v2/tailnet/-/keys {body}"));
        reply(&fake, &headers, json!({ "key": MINTED_KEY }))
    }

    async fn devices(State(fake): State<Fake>, headers: HeaderMap) -> Response {
        record(&fake, "GET /api/v2/tailnet/-/devices");
        let list = json!({ "devices": [
            { "id": "111", "hostname": "solo", "tags": ["tag:app"] },
            { "id": "222", "hostname": "twin", "tags": ["tag:app"] },
            { "id": "333", "hostname": "Twin", "tags": ["tag:app"] },
            { "id": "444", "tags": ["tag:app"] },
            { "id": "555", "hostname": "laptop" },
            { "id": "a/b", "hostname": "evil", "tags": ["tag:app"] },
        ]});
        reply(&fake, &headers, list)
    }

    async fn remove(
        State(fake): State<Fake>,
        headers: HeaderMap,
        Path(device): Path<String>,
    ) -> Response {
        record(&fake, &format!("DELETE /api/v2/device/{device}"));
        match device.as_str() {
            "gone" if authorized(&headers) => StatusCode::NOT_FOUND.into_response(),
            _ => reply(&fake, &headers, json!({})),
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

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn credentials(secret: &str) -> OAuthCredentials {
        OAuthCredentials::new(ClientId::new(CLIENT_ID), ClientSecret::new(secret))
    }

    async fn serve(behavior: Behavior) -> Served {
        let fake = Fake {
            log: Arc::default(),
            behavior,
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let root = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(axum::serve(listener, router(fake.clone())).into_future());
        Served { root, fake }
    }

    async fn harness_with(behavior: Behavior, secret: &str) -> Harness {
        let served = serve(behavior).await;
        let api = TailscaleApi::with_base(BaseUrl::new(served.root), credentials(secret)).unwrap();
        Harness {
            api,
            fake: served.fake,
        }
    }

    async fn harness() -> Harness {
        harness_with(Behavior::Normal, CLIENT_SECRET).await
    }

    fn tags(names: &[&str]) -> TailnetTags {
        names
            .iter()
            .map(|name| TailnetTag::new(*name).unwrap())
            .collect()
    }

    fn app_tags() -> TailnetTags {
        tags(&["tag:app"])
    }

    fn create_of(line: &str) -> Value {
        let body: Value = serde_json::from_str(line.split_once("keys ").unwrap().1).unwrap();
        body["capabilities"]["devices"]["create"].clone()
    }

    fn by_hostname(name: &str) -> DeviceRef {
        DeviceRef::Hostname(HostName::new(name).unwrap())
    }

    fn by_id(device: &str) -> DeviceRef {
        DeviceRef::Identifier(DeviceId::new(device).unwrap())
    }

    fn deleted_nothing(fake: &Fake) -> bool {
        lines(fake).iter().all(|line| !line.starts_with("DELETE"))
    }

    fn form_fields(request: &str) -> Vec<String> {
        let body = request.rsplit(' ').next().unwrap();
        let mut fields: Vec<String> = body.split('&').map(str::to_owned).collect();
        fields.sort();
        fields
    }

    fn leaks(text: &str) -> bool {
        text.contains(CLIENT_SECRET) || text.contains(MINTED_KEY)
    }

    #[test]
    fn a_key_is_single_use_preauthorized_non_ephemeral_and_tagged() {
        let runtime = runtime();
        let harness = runtime.block_on(harness());
        let key = runtime.block_on(harness.api.mint(&tags(&["tag:app", "tag:web"])));
        assert_eq!(key.unwrap().expose(), MINTED_KEY);
        let seen = lines(&harness.fake);
        assert_eq!(seen.len(), 2);
        let create = create_of(&seen[1]);
        assert_eq!(create["reusable"], json!(false));
        assert_eq!(create["ephemeral"], json!(false));
        assert_eq!(create["preauthorized"], json!(true));
        assert_eq!(create["tags"], json!(["tag:app", "tag:web"]));
    }

    #[test]
    fn the_token_asks_only_for_the_scope_of_the_key() {
        let runtime = runtime();
        let harness = runtime.block_on(harness());
        runtime.block_on(harness.api.mint(&app_tags())).unwrap();
        let expected = [
            "client_id=client-1",
            "client_secret=tskey-client-supersecret",
            "grant_type=client_credentials",
            "scope=auth_keys",
        ];
        assert_eq!(form_fields(&lines(&harness.fake)[0]), expected);
    }

    #[test]
    fn the_token_asks_only_for_the_scope_of_deleting() {
        let runtime = runtime();
        let harness = runtime.block_on(harness());
        runtime.block_on(harness.api.remove(&by_id("111"))).unwrap();
        let fields = form_fields(&lines(&harness.fake)[0]);
        assert!(fields.contains(&"scope=devices%3Acore".to_owned()));
    }

    #[test]
    fn bad_credentials_are_rejected_without_leaking_the_secret() {
        let runtime = runtime();
        let harness = runtime.block_on(harness_with(Behavior::Normal, "wrong-secret"));
        let error = runtime.block_on(harness.api.mint(&app_tags())).unwrap_err();
        assert!(matches!(error, TailnetError::Rejected(_)));
        assert!(!error.to_string().contains("wrong-secret"));
        assert!(!format!("{:?}", harness.api).contains("wrong-secret"));
    }

    #[test]
    fn credentials_never_print_the_secret() {
        let secret = ClientSecret::new(CLIENT_SECRET);
        let both = credentials(CLIENT_SECRET);
        assert!(!leaks(&format!("{secret:?}")));
        assert!(!leaks(&format!("{both:?}")));
        let runtime = runtime();
        let harness = runtime.block_on(harness());
        assert!(!leaks(&format!("{:?}", harness.api)));
    }

    #[test]
    fn a_refused_call_is_rejected_whatever_the_endpoint() {
        let runtime = runtime();
        let refusal = Behavior::Status(StatusCode::FORBIDDEN);
        let harness = runtime.block_on(harness_with(refusal, CLIENT_SECRET));
        let minted = runtime.block_on(harness.api.mint(&app_tags()));
        let by_id = runtime.block_on(harness.api.remove(&by_id("111")));
        let named = runtime.block_on(harness.api.remove(&by_hostname("solo")));
        assert!(matches!(minted, Err(TailnetError::Rejected(_))));
        assert!(matches!(by_id, Err(TailnetError::Rejected(_))));
        assert!(matches!(named, Err(TailnetError::Rejected(_))));
    }

    #[test]
    fn a_missing_route_is_not_a_missing_device() {
        let runtime = runtime();
        let missing = Behavior::Status(StatusCode::NOT_FOUND);
        let harness = runtime.block_on(harness_with(missing, CLIENT_SECRET));
        let minted = runtime.block_on(harness.api.mint(&app_tags()));
        let listed = runtime.block_on(harness.api.remove(&by_hostname("solo")));
        assert!(matches!(minted, Err(TailnetError::Rejected(_))));
        assert!(matches!(listed, Err(TailnetError::Rejected(_))));
    }

    #[test]
    fn an_unreadable_answer_fails_without_echoing_anything() {
        let runtime = runtime();
        let harness = runtime.block_on(harness_with(Behavior::Malformed, CLIENT_SECRET));
        let error = runtime.block_on(harness.api.mint(&app_tags())).unwrap_err();
        assert!(matches!(error, TailnetError::Unavailable(_)));
        assert!(!leaks(&error.to_string()));
        assert!(!leaks(&format!("{error:?}")));
    }

    #[test]
    fn an_unreadable_device_list_fails_without_deleting() {
        let runtime = runtime();
        let harness = runtime.block_on(harness_with(Behavior::Malformed, CLIENT_SECRET));
        let error = runtime
            .block_on(harness.api.remove(&by_hostname("solo")))
            .unwrap_err();
        assert!(matches!(error, TailnetError::Unavailable(_)));
        assert!(deleted_nothing(&harness.fake));
    }

    async fn closed_port() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    }

    #[test]
    fn an_unreachable_server_is_unavailable() {
        let runtime = runtime();
        let root = runtime.block_on(closed_port());
        let closed = TailscaleApi::with_base(BaseUrl::new(root), credentials(CLIENT_SECRET));
        let result = runtime.block_on(closed.unwrap().mint(&app_tags()));
        assert!(matches!(result, Err(TailnetError::Unavailable(_))));
    }

    #[test]
    fn an_answer_over_the_size_cap_is_refused() {
        let runtime = runtime();
        let harness = runtime.block_on(harness_with(Behavior::Oversized, CLIENT_SECRET));
        let error = runtime.block_on(harness.api.mint(&app_tags())).unwrap_err();
        assert!(matches!(error, TailnetError::Unavailable(_)));
        assert!(!error.to_string().contains("kkkk"));
    }

    #[test]
    fn a_redirect_is_not_followed_so_the_secret_goes_nowhere_else() {
        let runtime = runtime();
        let elsewhere = runtime.block_on(serve(Behavior::Normal));
        let target = format!("{}/api/v2/oauth/token", elsewhere.root);
        let harness = runtime.block_on(harness_with(Behavior::Redirect(target), CLIENT_SECRET));
        let result = runtime.block_on(harness.api.mint(&app_tags()));
        assert!(matches!(result, Err(TailnetError::Rejected(_))));
        assert!(lines(&elsewhere.fake).is_empty());
    }

    #[test]
    fn a_base_url_that_would_expose_the_secret_is_refused() {
        let plain = BaseUrl::new("http://tailscale.example");
        let result = TailscaleApi::with_base(plain, credentials(CLIENT_SECRET));
        assert_eq!(result.unwrap_err(), TailnetError::InsecureBase);
    }

    #[test]
    fn a_device_is_deleted_by_id() {
        let runtime = runtime();
        let harness = runtime.block_on(harness());
        runtime.block_on(harness.api.remove(&by_id("111"))).unwrap();
        let seen = lines(&harness.fake);
        assert!(seen[0].starts_with("POST /api/v2/oauth/token"));
        assert_eq!(seen[1], "DELETE /api/v2/device/111");
    }

    #[test]
    fn a_device_is_deleted_by_its_only_hostname_match_ignoring_case() {
        let runtime = runtime();
        let harness = runtime.block_on(harness());
        runtime
            .block_on(harness.api.remove(&by_hostname("SOLO")))
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
        assert!(deleted_nothing(&harness.fake));
    }

    #[test]
    fn an_untagged_device_is_never_matched_by_hostname() {
        let runtime = runtime();
        let harness = runtime.block_on(harness());
        let result = runtime.block_on(harness.api.remove(&by_hostname("laptop")));
        assert_eq!(result, Err(TailnetError::NoSuchDevice));
        assert!(deleted_nothing(&harness.fake));
    }

    #[test]
    fn a_hostile_device_id_never_reaches_a_path() {
        let runtime = runtime();
        let harness = runtime.block_on(harness());
        let result = runtime.block_on(harness.api.remove(&by_hostname("evil")));
        assert!(matches!(result, Err(TailnetError::Unavailable(_))));
        assert!(deleted_nothing(&harness.fake));
    }

    #[test]
    fn a_device_that_is_already_gone_is_reported() {
        let runtime = runtime();
        let harness = runtime.block_on(harness());
        let result = runtime.block_on(harness.api.remove(&by_id("gone")));
        assert_eq!(result, Err(TailnetError::NoSuchDevice));
    }

    fn app_node() -> HostName {
        HostName::new("app").unwrap()
    }

    fn enroll(
        runtime: &tokio::runtime::Runtime,
        harness: Harness,
        volume: StateVolume,
    ) -> Result<Enrolled, EnrollError> {
        let enrollment = Enrollment::new(Volume(volume), harness.api);
        runtime.block_on(enrollment.enroll(&app_node(), &app_tags()))
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
