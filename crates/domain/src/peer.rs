//! Who a tailnet peer is, as far as access control cares.

use crate::policy::TailnetTags;
use crate::value::{TailnetTag, ValueError};

/// Which side of the controller-agent API a node is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AegisRole {
    /// The controller.
    Controller,
    /// A per-host agent.
    Agent,
}

impl AegisRole {
    /// The tag a node of this role carries in the tailnet.
    ///
    /// # Errors
    ///
    /// Returns a [`ValueError`] only if the built-in tag name were invalid.
    pub fn tag(self) -> Result<TailnetTag, ValueError> {
        match self {
            Self::Controller => TailnetTag::new("tag:aegis-controller"),
            Self::Agent => TailnetTag::new("tag:aegis-agent"),
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

/// A tailnet node as the Tailscale local API resolved it: the ACL tags it carries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeerIdentity(TailnetTags);

impl PeerIdentity {
    /// A peer carrying `tags`; an untagged node carries none.
    #[must_use]
    pub const fn new(tags: TailnetTags) -> Self {
        Self(tags)
    }

    /// Whether the peer carries `tag`.
    #[must_use]
    pub fn has_tag(&self, tag: &TailnetTag) -> bool {
        self.0.iter().any(|candidate| candidate == tag)
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

    #[test]
    fn an_untagged_peer_has_no_tag() {
        assert!(!PeerIdentity::default().has_tag(&tag("tag:aegis-agent")));
    }
}
