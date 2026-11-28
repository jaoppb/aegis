//! Who a tailnet peer is, as far as access control cares.

use std::fmt;

use crate::policy::TailnetTags;
use crate::value::{TailnetTag, ValueError, collection};

/// Which side of the controller-agent API a node is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AegisRole {
    /// The controller.
    Controller,
    /// A per-host agent.
    Agent,
}

const CONTROLLER_TAG: &str = "tag:aegis-controller";
const AGENT_TAG: &str = "tag:aegis-agent";

impl AegisRole {
    /// Whether `tag` is the tag of an Aegis role, which the API authorizer trusts.
    #[must_use]
    pub fn is_role_tag(tag: &TailnetTag) -> bool {
        matches!(tag.as_str(), CONTROLLER_TAG | AGENT_TAG)
    }

    /// The tag a node of this role carries in the tailnet.
    ///
    /// # Errors
    ///
    /// Returns a [`ValueError`] only if the built-in tag name were invalid.
    pub fn tag(self) -> Result<TailnetTag, ValueError> {
        match self {
            Self::Controller => TailnetTag::new(CONTROLLER_TAG),
            Self::Agent => TailnetTag::new(AGENT_TAG),
        }
    }

    /// The tag a caller must carry to reach a server with this role: the other side's tag.
    ///
    /// # Errors
    ///
    /// Returns a [`ValueError`] only if the built-in tag name were invalid.
    pub fn caller_tag(self) -> Result<TailnetTag, ValueError> {
        match self {
            Self::Controller => Self::Agent.tag(),
            Self::Agent => Self::Controller.tag(),
        }
    }
}

/// The Tailscale login of the person who owns a node, such as `ada@example.com`. Logins compare
/// without regard to ASCII case.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Login(String);

impl Login {
    /// The login `value`, lower-cased.
    ///
    /// # Errors
    ///
    /// Returns [`ValueError::Empty`] for a blank value and [`ValueError::ForbiddenChar`] for one
    /// with whitespace, a control character, a non-ASCII character or a comma (the separator of
    /// a list of logins).
    pub fn new(value: &str) -> Result<Self, ValueError> {
        let trimmed = value.trim();
        check_login(trimmed)?;
        Ok(Self(trimmed.to_ascii_lowercase()))
    }

    /// The login, lower-cased.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A login is not blank and holds only printable ASCII without whitespace or a comma: Unicode
/// case folding could make two different logins compare equal.
fn check_login(login: &str) -> Result<(), ValueError> {
    if login.is_empty() {
        return Err(ValueError::Empty);
    }
    let forbidden = login
        .chars()
        .find(|found| !found.is_ascii_graphic() || *found == ',');
    forbidden.map_or(Ok(()), |found| Err(ValueError::ForbiddenChar(found)))
}

impl fmt::Display for Login {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

collection!(
    /// The logins that may operate Aegis.
    Logins,
    Login
);

impl Logins {
    /// The logins of a comma-separated list such as `ada@example.com, bob@example.com`; blank
    /// items are skipped, so an empty list names nobody.
    ///
    /// # Errors
    ///
    /// Returns a [`ValueError`] when an item is not a valid login.
    pub fn parse_list(list: &str) -> Result<Self, ValueError> {
        let names = list.split(',').map(str::trim);
        let present = names.filter(|name| !name.is_empty());
        present.map(Login::new).collect()
    }

    /// Whether `login` is one of them.
    #[must_use]
    pub fn contains(&self, login: &Login) -> bool {
        self.iter().any(|candidate| candidate == login)
    }
}

/// A tailnet node as the Tailscale local API resolved it: the ACL tags it carries and, for a node
/// owned by a person, the login of that person.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeerIdentity {
    tags: TailnetTags,
    login: Option<Login>,
}

impl PeerIdentity {
    /// A peer carrying `tags`; an untagged node carries none.
    #[must_use]
    pub const fn new(tags: TailnetTags) -> Self {
        Self { tags, login: None }
    }

    /// The same peer, owned by the person with `login`.
    #[must_use]
    pub fn with_login(self, login: Login) -> Self {
        Self {
            login: Some(login),
            ..self
        }
    }

    /// Whether the peer carries `tag`.
    #[must_use]
    pub fn has_tag(&self, tag: &TailnetTag) -> bool {
        self.tags.iter().any(|candidate| candidate == tag)
    }

    /// Whether the peer is an operator: a node of a person in `operators`. A tagged node is owned
    /// by the tailnet, not by a person, so it is never an operator whatever login it reports.
    #[must_use]
    pub fn is_operator(&self, operators: &Logins) -> bool {
        let owner = self.login.as_ref();
        self.tags.is_empty() && owner.is_some_and(|login| operators.contains(login))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(name: &str) -> TailnetTag {
        TailnetTag::new(name).unwrap()
    }

    #[test]
    fn a_peer_has_exactly_the_tags_it_carries() {
        let peer = PeerIdentity::new(TailnetTags::new(vec![tag("tag:aegis-agent")]));
        assert!(peer.has_tag(&tag("tag:aegis-agent")));
        assert!(!peer.has_tag(&tag("tag:aegis-controller")));
    }

    #[test]
    fn only_the_role_tags_are_role_tags() {
        assert!(AegisRole::is_role_tag(&tag("tag:aegis-agent")));
        assert!(AegisRole::is_role_tag(&tag("tag:aegis-controller")));
        assert!(!AegisRole::is_role_tag(&tag("tag:app")));
    }

    #[test]
    fn every_role_has_a_valid_tag() {
        assert_eq!(AegisRole::Agent.tag().unwrap().as_str(), "tag:aegis-agent");
        let controller = AegisRole::Controller.tag().unwrap();
        assert_eq!(controller.as_str(), "tag:aegis-controller");
    }

    #[test]
    fn a_server_is_called_by_the_other_side() {
        let agent_server = AegisRole::Agent.caller_tag().unwrap();
        let controller_server = AegisRole::Controller.caller_tag().unwrap();
        assert_eq!(agent_server, AegisRole::Controller.tag().unwrap());
        assert_eq!(controller_server, AegisRole::Agent.tag().unwrap());
    }

    fn operators() -> Logins {
        Logins::parse_list("Ada@Example.com, bob@example.com,").unwrap()
    }

    fn login(text: &str) -> Login {
        Login::new(text).unwrap()
    }

    #[test]
    fn logins_compare_without_regard_to_case() {
        assert_eq!(login("ADA@example.com"), login("ada@example.com"));
        assert_eq!(operators().len(), 2);
    }

    #[test]
    fn a_login_with_whitespace_or_a_comma_is_refused() {
        assert_eq!(Login::new("  "), Err(ValueError::Empty));
        assert!(Login::new("ada lovelace").is_err());
        assert!(Logins::parse_list("ada@x,b d").is_err());
    }

    #[test]
    fn a_login_with_a_non_ascii_character_is_refused() {
        assert!(Login::new("\u{212A}@example.com").is_err());
    }

    #[test]
    fn an_empty_list_names_nobody() {
        assert!(Logins::parse_list(" , ").unwrap().is_empty());
    }

    #[test]
    fn an_untagged_node_of_a_listed_login_is_an_operator() {
        let peer = PeerIdentity::default().with_login(login("ada@example.com"));
        assert!(peer.is_operator(&operators()));
    }

    #[test]
    fn an_untagged_node_of_an_unlisted_login_is_not_an_operator() {
        let peer = PeerIdentity::default().with_login(login("eve@example.com"));
        assert!(!peer.is_operator(&operators()));
        assert!(!PeerIdentity::default().is_operator(&operators()));
    }

    #[test]
    fn a_tagged_node_is_never_an_operator_whatever_its_login() {
        let tags = TailnetTags::new(vec![tag("tag:aegis-agent")]);
        let peer = PeerIdentity::new(tags).with_login(login("ada@example.com"));
        assert!(!peer.is_operator(&operators()));
    }

    #[test]
    fn an_untagged_peer_has_no_tag() {
        assert!(!PeerIdentity::default().has_tag(&tag("tag:aegis-agent")));
    }
}
