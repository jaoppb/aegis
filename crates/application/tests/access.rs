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
    use aegis_application::{
        AccessError, Admit, Authorizer, Caller, PeerAddress, Reason, WhoIsError,
    };
    use aegis_domain::{HostName, Login, Logins, PeerIdentity, TailnetTag, TailnetTags};

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

    #[test]
    fn a_tagged_caller_is_an_agent_with_its_node_and_a_person_is_an_operator() {
        let authorizer = Authorizer::new(peer_with(&[]), tag("tag:aegis-agent"));
        let tags = TailnetTags::new(vec![tag("tag:aegis-agent")]);
        let node = HostName::new("jao-server").unwrap();
        let agent = PeerIdentity::new(tags).with_node(node.clone());
        let caller = authorizer.caller_of(&agent);
        assert_eq!(caller.node(), Some(&node));
        let person = PeerIdentity::default().with_login(Login::new("ada@example.com").unwrap());
        assert_eq!(authorizer.caller_of(&person), Caller::Operator);
        assert_eq!(Caller::Operator.node(), None);
    }

    fn hosts_of(node: &str) -> Vec<String> {
        let tags = TailnetTags::new(vec![tag("tag:aegis-agent")]);
        let identity = PeerIdentity::new(tags).with_node(HostName::new(node).unwrap());
        let names = Caller::Agent(identity).hosts();
        names.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn an_agent_node_stands_for_the_host_it_is_named_after_or_bootstrapped_for() {
        assert_eq!(hosts_of("jao-server"), ["jao-server"]);
        let bootstrapped = hosts_of("jao-server-aegis-agent");
        assert_eq!(bootstrapped, ["jao-server-aegis-agent", "jao-server"]);
        assert!(Caller::Operator.hosts().is_empty());
    }
}
