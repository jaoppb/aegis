//! The `/v1` API over a real socket, with a scripted `WhoIs` standing in for Tailscale.

use std::future::{Future, ready};
use std::sync::{Arc, Mutex};

use aegis_application::{PeerAddress, WhoIs, WhoIsError};
use aegis_domain::PeerIdentity;

struct Scripted {
    answer: Result<PeerIdentity, WhoIsError>,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Scripted {
    fn answering(answer: Result<PeerIdentity, WhoIsError>) -> Self {
        Self {
            answer,
            seen: Arc::default(),
        }
    }
}

impl WhoIs for Scripted {
    fn identify(
        &self,
        peer: &PeerAddress,
    ) -> impl Future<Output = Result<PeerIdentity, WhoIsError>> + Send {
        let record = |mut seen: std::sync::MutexGuard<'_, Vec<String>>| seen.push(peer.to_string());
        self.seen.lock().into_iter().for_each(record);
        ready(self.answer.clone())
    }
}

#[cfg(test)]
mod cases {
    use super::Scripted;
    use aegis_adapters::api::{ApiClient, BaseUrl, ClientError, serve};
    use aegis_api::version_1::{Health, Role};
    use aegis_application::{Authorizer, Reason, WhoIsError};
    use std::sync::Arc;

    use aegis_domain::{AegisRole, PeerIdentity, TailnetTag, TailnetTags};
    use reqwest::StatusCode;
    use tokio::net::TcpListener;

    fn tagged(names: &[&str]) -> Scripted {
        let tags = names.iter().map(|name| TailnetTag::new(*name).unwrap());
        Scripted::answering(Ok(PeerIdentity::new(tags.collect::<TailnetTags>())))
    }

    const fn role_of(role: Role) -> AegisRole {
        match role {
            Role::Agent => AegisRole::Agent,
            Role::Controller => AegisRole::Controller,
        }
    }

    async fn call(role: Role, lookup: Scripted) -> Result<Health, ClientError> {
        let authorizer = Authorizer::for_server(lookup, role_of(role)).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = BaseUrl::new(format!("http://{}/", listener.local_addr().unwrap()));
        tokio::spawn(serve(listener, role, authorizer));
        ApiClient::new(base).unwrap().health().await
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    /// Start a server with `role` that resolves every peer to `lookup`, and call its health
    /// endpoint.
    fn health_of(role: Role, lookup: Scripted) -> Result<Health, ClientError> {
        runtime().block_on(call(role, lookup))
    }

    fn status_of(result: Result<Health, ClientError>) -> StatusCode {
        match result {
            Err(ClientError::Rejected(status)) => status,
            other => panic!("expected a rejection, got {other:?}"),
        }
    }

    #[test]
    fn an_agent_answers_a_tagged_controller() {
        let health = health_of(Role::Agent, tagged(&["tag:aegis-controller"]));
        assert_eq!(health.unwrap(), Health::of(Role::Agent));
    }

    #[test]
    fn a_controller_answers_a_tagged_agent() {
        let health = health_of(Role::Controller, tagged(&["tag:aegis-agent"]));
        assert_eq!(health.unwrap(), Health::of(Role::Controller));
    }

    #[test]
    fn an_agent_rejects_an_untagged_node() {
        let status = status_of(health_of(Role::Agent, tagged(&[])));
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[test]
    fn an_agent_rejects_another_agent() {
        let status = status_of(health_of(Role::Agent, tagged(&["tag:aegis-agent"])));
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[test]
    fn a_controller_rejects_another_controller() {
        let wrong = tagged(&["tag:aegis-controller"]);
        let status = status_of(health_of(Role::Controller, wrong));
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[test]
    fn an_unknown_address_is_forbidden() {
        let unknown = Scripted::answering(Err(WhoIsError::Unknown));
        let status = status_of(health_of(Role::Agent, unknown));
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[test]
    fn a_failed_lookup_fails_closed() {
        let failed = Scripted::answering(Err(WhoIsError::Unavailable(Reason::new(&"daemon down"))));
        let status = status_of(health_of(Role::Agent, failed));
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }

    async fn peers_seen_by_whois() -> (Vec<String>, u16) {
        let lookup = tagged(&["tag:aegis-controller"]);
        let seen = Arc::clone(&lookup.seen);
        let authorizer = Authorizer::for_server(lookup, AegisRole::Agent).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let base = BaseUrl::new(format!("http://127.0.0.1:{port}"));
        tokio::spawn(serve(listener, Role::Agent, authorizer));
        ApiClient::new(base).unwrap().health().await.unwrap();
        let recorded = seen.lock().unwrap().clone();
        (recorded, port)
    }

    #[test]
    fn whois_receives_the_tcp_source_address_of_the_caller() {
        let (peers, port) = runtime().block_on(peers_seen_by_whois());
        assert_eq!(peers.len(), 1);
        let address: std::net::SocketAddr = peers[0].parse().unwrap();
        assert!(address.ip().is_loopback());
        assert_ne!(address.port(), port);
    }

    async fn status_of_get(path: &str) -> StatusCode {
        let authorizer = Authorizer::for_server(tagged(&[]), AegisRole::Agent).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}{path}", listener.local_addr().unwrap());
        tokio::spawn(serve(listener, Role::Agent, authorizer));
        reqwest::get(url).await.unwrap().status()
    }

    #[test]
    fn a_denied_peer_gets_no_answer_on_any_path() {
        let unknown = runtime().block_on(status_of_get("/v1/other"));
        assert_eq!(unknown, StatusCode::FORBIDDEN);
    }
}
