//! The `/v1` API over a real socket, with a scripted `WhoIs` standing in for Tailscale.

use std::future::{Future, ready};

use aegis_application::{PeerAddress, WhoIs, WhoIsError};
use aegis_domain::PeerIdentity;

struct Scripted(Result<PeerIdentity, WhoIsError>);

impl WhoIs for Scripted {
    fn identify(
        &self,
        _peer: &PeerAddress,
    ) -> impl Future<Output = Result<PeerIdentity, WhoIsError>> + Send {
        ready(self.0.clone())
    }
}

#[cfg(test)]
mod cases {
    use super::Scripted;
    use aegis_adapters::api::{ApiClient, BaseUrl, ClientError, serve};
    use aegis_api::version_1::{Health, Role};
    use aegis_application::{Authorizer, Reason, WhoIsError};
    use aegis_domain::{PeerIdentity, TailnetTag, TailnetTags};
    use reqwest::StatusCode;
    use tokio::net::TcpListener;

    fn tagged(names: &[&str]) -> Scripted {
        let tags = names.iter().map(|name| TailnetTag::new(*name).unwrap());
        Scripted(Ok(PeerIdentity::new(tags.collect::<TailnetTags>())))
    }

    async fn call(role: Role, lookup: Scripted) -> Result<Health, ClientError> {
        let required = match role {
            Role::Agent => "tag:aegis-controller",
            Role::Controller => "tag:aegis-agent",
        };
        let authorizer = Authorizer::new(lookup, TailnetTag::new(required).unwrap());
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
        let unknown = Scripted(Err(WhoIsError::Unknown));
        let status = status_of(health_of(Role::Agent, unknown));
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[test]
    fn a_failed_lookup_fails_closed() {
        let failed = Scripted(Err(WhoIsError::Unavailable(Reason::new(&"daemon down"))));
        let status = status_of(health_of(Role::Agent, failed));
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }
}
