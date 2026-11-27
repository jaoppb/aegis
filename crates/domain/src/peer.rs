//! Who a tailnet peer is, as far as access control cares.

use crate::policy::TailnetTags;
use crate::value::TailnetTag;

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
    fn an_untagged_peer_has_no_tag() {
        assert!(!PeerIdentity::default().has_tag(&tag("tag:aegis-agent")));
    }
}
