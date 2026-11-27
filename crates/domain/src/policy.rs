//! The per-stack policy declared in an optional `aegis.toml`.

use crate::value::{Port, ServiceName, TailnetTag, collection};

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
    expose: Option<Expose>,
    update: UpdatePolicy,
    tailnet_tags: TailnetTags,
    state: StackState,
}

impl StackPolicy {
    /// A policy with the default update policy.
    #[must_use]
    pub fn new(expose: Option<Expose>, tailnet_tags: TailnetTags, state: StackState) -> Self {
        Self {
            expose,
            update: UpdatePolicy::default(),
            tailnet_tags,
            state,
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
        self.expose.as_ref()
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
