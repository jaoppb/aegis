//! The per-stack policy declared in an optional `aegis.toml`.

use crate::value::{Port, ServiceName, TailnetTag, ValueError, collection};

/// The service and port Aegis exposes on the tailnet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expose {
    service: ServiceName,
    port: Port,
}

impl Expose {
    /// Expose `service` on `port`.
    #[must_use]
    pub const fn new(service: ServiceName, port: Port) -> Self {
        Self { service, port }
    }

    /// The compose service to expose.
    #[must_use]
    pub const fn service(&self) -> &ServiceName {
        &self.service
    }

    /// The port the service listens on.
    #[must_use]
    pub const fn port(&self) -> Port {
        self.port
    }
}

/// The service that joins the stack's own tailnet node for raw TCP and UDP, with no Caddy in front.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    service: ServiceName,
}

impl Node {
    /// A node that `service` joins.
    #[must_use]
    pub const fn new(service: ServiceName) -> Self {
        Self { service }
    }

    /// The compose service that shares the node's network namespace.
    #[must_use]
    pub const fn service(&self) -> &ServiceName {
        &self.service
    }
}

/// How a stack reaches the tailnet. A stack has at most one node, so the two ways exclude each
/// other.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum Reach {
    /// The stack has no tailnet node.
    #[default]
    Private,
    /// HTTPS through Caddy.
    Https(Expose),
    /// Raw TCP and UDP.
    Raw(Node),
}

/// How image updates are applied. A placeholder until the update policy is designed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum UpdatePolicy {
    /// Updates are never proposed.
    #[default]
    Manual,
}

/// The lifecycle state of a stack.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StackState {
    /// The stack runs.
    #[default]
    Active,
    /// A tombstone: stop the stack and delete its volumes and tailnet node.
    Purged,
}

collection!(
    /// The ACL tags for a tailnet node.
    TailnetTags,
    TailnetTag
);

/// Everything `aegis.toml` can say about a stack. A missing file means all defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StackPolicy {
    reach: Reach,
    update: UpdatePolicy,
    tailnet_tags: TailnetTags,
    state: StackState,
}

impl StackPolicy {
    /// A policy with the default update policy.
    #[must_use]
    pub fn new(expose: Option<Expose>, tailnet_tags: TailnetTags, state: StackState) -> Self {
        Self {
            reach: expose.map_or(Reach::Private, Reach::Https),
            update: UpdatePolicy::default(),
            tailnet_tags,
            state,
        }
    }

    /// The same policy whose tailnet node `node` serves raw TCP and UDP, instead of any exposure.
    #[must_use]
    pub fn with_node(self, node: Node) -> Self {
        Self {
            reach: Reach::Raw(node),
            ..self
        }
    }

    /// The same policy with `update` as its update policy.
    #[must_use]
    pub fn with_update(self, update: UpdatePolicy) -> Self {
        Self { update, ..self }
    }

    /// What to expose on the tailnet, if anything.
    #[must_use]
    pub const fn expose(&self) -> Option<&Expose> {
        match &self.reach {
            Reach::Https(expose) => Some(expose),
            Reach::Private | Reach::Raw(_) => None,
        }
    }

    /// The raw tailnet node, if the stack asks for one.
    #[must_use]
    pub const fn node(&self) -> Option<&Node> {
        match &self.reach {
            Reach::Raw(node) => Some(node),
            Reach::Private | Reach::Https(_) => None,
        }
    }

    /// The service whose name is the tailnet hostname, whichever way the stack reaches the
    /// tailnet.
    #[must_use]
    pub const fn tailnet_service(&self) -> Option<&ServiceName> {
        match &self.reach {
            Reach::Https(expose) => Some(expose.service()),
            Reach::Raw(node) => Some(node.service()),
            Reach::Private => None,
        }
    }

    /// How image updates are applied.
    #[must_use]
    pub const fn update(&self) -> UpdatePolicy {
        self.update
    }

    /// ACL tags for the tailnet node.
    #[must_use]
    pub const fn tailnet_tags(&self) -> &TailnetTags {
        &self.tailnet_tags
    }

    /// The lifecycle state.
    #[must_use]
    pub const fn state(&self) -> StackState {
        self.state
    }
}

impl TailnetTags {
    /// The tags of a comma-separated list such as `tag:a, tag:b`; blank items are skipped.
    ///
    /// # Errors
    ///
    /// Returns a [`ValueError`] when an item is not a valid tag.
    pub fn parse_list(list: &str) -> Result<Self, ValueError> {
        let names = list.split(',').map(str::trim);
        let present = names.filter(|name| !name.is_empty());
        let tags: Result<Vec<TailnetTag>, ValueError> = present.map(TailnetTag::new).collect();
        tags.map(Self::new)
    }

    /// The tags as a comma-separated list, the inverse of [`TailnetTags::parse_list`].
    #[must_use]
    pub fn join(&self) -> String {
        let names: Vec<&str> = self.iter().map(TailnetTag::as_str).collect();
        names.join(",")
    }
}
