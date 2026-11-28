//! The authorizer against a scripted `WhoIs`.

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
    use aegis_application::{AccessError, Admit, Authorizer, PeerAddress, Reason, WhoIsError};
    use aegis_domain::{Login, Logins, PeerIdentity, TailnetTag, TailnetTags};

    fn tag(name: &str) -> TailnetTag {
        TailnetTag::new(name).unwrap()
    }

    fn peer_with(tags: &[&str]) -> Scripted {
        let tags = tags.iter().map(|name| tag(name)).collect::<TailnetTags>();
        Scripted(Ok(PeerIdentity::new(tags)))
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
    }

    fn verdict(lookup: Scripted) -> Result<(), AccessError> {
        let peer = PeerAddress::new("100.64.0.7:41234");
        let authorizer = Authorizer::new(lookup, tag("tag:aegis-agent"));
        runtime().block_on(authorizer.authorize(&peer))
    }

    #[test]
    fn a_peer_with_the_required_tag_is_let_through() {
        assert_eq!(verdict(peer_with(&["tag:aegis-agent"])), Ok(()));
    }

    #[test]
    fn a_peer_with_several_tags_passes_when_one_matches() {
        let peer = peer_with(&["tag:other", "tag:aegis-agent"]);
        assert_eq!(verdict(peer), Ok(()));
    }

    #[test]
    fn an_untagged_peer_is_denied() {
        assert_eq!(verdict(peer_with(&[])), Err(AccessError::Denied));
    }

    #[test]
    fn a_wrongly_tagged_peer_is_denied() {
        let wrong = peer_with(&["tag:aegis-controller"]);
        assert_eq!(verdict(wrong), Err(AccessError::Denied));
    }

    #[test]
    fn an_unknown_address_is_denied() {
        let unknown = Scripted(Err(WhoIsError::Unknown));
        assert_eq!(verdict(unknown), Err(AccessError::Denied));
    }

    #[test]
    fn a_failed_lookup_fails_closed() {
        let reason = Reason::new(&"daemon down");
        let failed = Scripted(Err(WhoIsError::Unavailable(reason.clone())));
        assert_eq!(verdict(failed), Err(AccessError::Unavailable(reason)));
    }

    fn person(login: &str) -> Scripted {
        let identity = PeerIdentity::default().with_login(Login::new(login).unwrap());
        Scripted(Ok(identity))
    }

    fn listed(text: &str) -> Logins {
        Logins::parse_list(text).unwrap()
    }

    fn verdict_for(lookup: Scripted, admit: Admit, operators: &Logins) -> Result<(), AccessError> {
        let peer = PeerAddress::new("100.64.0.7:41234");
        let authorizer =
            Authorizer::new(lookup, tag("tag:aegis-agent")).with_operators(operators.clone());
        runtime().block_on(authorizer.authorize_for(&peer, admit))
    }

    #[test]
    fn a_listed_operator_passes_where_operators_are_admitted() {
        let lookup = person("ada@example.com");
        assert_eq!(
            verdict_for(lookup, Admit::Operator, &listed("ada@example.com")),
            Ok(())
        );
        let lookup = person("ada@example.com");
        assert_eq!(
            verdict_for(lookup, Admit::Either, &listed("ada@example.com")),
            Ok(())
        );
    }

    #[test]
    fn an_operator_is_denied_where_only_the_tag_is_admitted() {
        let lookup = person("ada@example.com");
        let tagged = verdict_for(lookup, Admit::Tagged, &listed("ada@example.com"));
        assert_eq!(tagged, Err(AccessError::Denied));
    }

    #[test]
    fn a_tagged_peer_is_denied_where_only_operators_are_admitted() {
        let agent = peer_with(&["tag:aegis-agent"]);
        assert_eq!(
            verdict_for(agent, Admit::Operator, &listed("ada@example.com")),
            Err(AccessError::Denied)
        );
    }

    #[test]
    fn an_unlisted_person_is_denied_everywhere() {
        let stranger = verdict_for(
            person("eve@example.com"),
            Admit::Either,
            &listed("ada@example.com"),
        );
        assert_eq!(stranger, Err(AccessError::Denied));
    }

    #[test]
    fn without_operators_nobody_is_admitted_as_one() {
        let lookup = person("ada@example.com");
        assert_eq!(
            verdict_for(lookup, Admit::Either, &listed("")),
            Err(AccessError::Denied)
        );
    }
}
