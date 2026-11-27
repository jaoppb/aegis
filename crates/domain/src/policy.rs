//! The per-stack policy declared in an optional `aegis.toml`.

use crate::value::{Port, ServiceName, TailnetTag};

/// The service and port Aegis exposes on the tailnet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expose {
    /// The compose service to expose.
    pub service: ServiceName,
    /// The port the service listens on.
    pub port: Port,
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

/// Everything `aegis.toml` can say about a stack. A missing file means all defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StackPolicy {
    /// What to expose on the tailnet, if anything.
    pub expose: Option<Expose>,
    /// How image updates are applied.
    pub update: UpdatePolicy,
    /// ACL tags for the tailnet node.
    pub tailnet_tags: Vec<TailnetTag>,
    /// The lifecycle state.
    pub state: StackState,
}
