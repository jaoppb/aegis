//! The desired state of a whole repository.

use crate::compose::ComposeSummary;
use crate::policy::StackPolicy;
use crate::value::{HostName, StackName};

/// Whether a stack belongs to a host or to Aegis itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackKind {
    /// A user stack under `hosts/<host>/`.
    Host,
    /// One of Aegis's own stacks under the reserved `aegis/` directory.
    Reserved,
}

/// One stack: a compose file and its policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stack {
    /// The stack name.
    pub name: StackName,
    /// Whether this is a user stack or a reserved Aegis stack.
    pub kind: StackKind,
    /// The policy from `aegis.toml`, or the defaults.
    pub policy: StackPolicy,
    /// The compose file exactly as written in the repository.
    pub compose_source: String,
    /// What the domain reasons about in the compose file.
    pub compose: ComposeSummary,
}

/// A host and the stacks assigned to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    /// The host name.
    pub name: HostName,
    /// The stacks, in name order.
    pub stacks: Vec<Stack>,
}

/// Everything the repository declares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DesiredState {
    /// The hosts, in name order.
    pub hosts: Vec<Host>,
    /// Aegis's own stacks from the reserved `aegis/` directory.
    pub reserved: Vec<Stack>,
}
