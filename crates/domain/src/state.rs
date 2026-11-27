//! The desired state of a whole repository.

use crate::compose::ComposeSummary;
use crate::policy::StackPolicy;
use crate::value::{ComposeSource, HostName, StackName, collection};

/// Whether a stack belongs to a host or to Aegis itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackKind {
    /// A user stack under `hosts/<host>/`.
    Host,
    /// One of Aegis's own stacks under the reserved `aegis/` directory.
    Reserved,
}

/// What a stack is called and what kind of stack it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackIdentity {
    name: StackName,
    kind: StackKind,
}

impl StackIdentity {
    /// The stack `name` of `kind`.
    #[must_use]
    pub const fn new(name: StackName, kind: StackKind) -> Self {
        Self { name, kind }
    }
}

/// What a stack declares: its policy and its compose file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackDefinition {
    policy: StackPolicy,
    compose_source: ComposeSource,
    compose: ComposeSummary,
}

impl StackDefinition {
    /// A definition from the policy, the compose text and what the domain reasons about in it.
    #[must_use]
    pub const fn new(
        policy: StackPolicy,
        compose_source: ComposeSource,
        compose: ComposeSummary,
    ) -> Self {
        Self {
            policy,
            compose_source,
            compose,
        }
    }
}

/// One stack: a compose file and its policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stack {
    identity: StackIdentity,
    definition: StackDefinition,
}

impl Stack {
    /// A stack with `identity` that declares `definition`.
    #[must_use]
    pub const fn new(identity: StackIdentity, definition: StackDefinition) -> Self {
        Self {
            identity,
            definition,
        }
    }

    /// The stack name.
    #[must_use]
    pub const fn name(&self) -> &StackName {
        &self.identity.name
    }

    /// Whether this is a user stack or a reserved Aegis stack.
    #[must_use]
    pub const fn kind(&self) -> StackKind {
        self.identity.kind
    }

    /// The policy from `aegis.toml`, or the defaults.
    #[must_use]
    pub const fn policy(&self) -> &StackPolicy {
        &self.definition.policy
    }

    /// The compose file exactly as written in the repository.
    #[must_use]
    pub const fn compose_source(&self) -> &ComposeSource {
        &self.definition.compose_source
    }

    /// What the domain reasons about in the compose file.
    #[must_use]
    pub const fn compose(&self) -> &ComposeSummary {
        &self.definition.compose
    }
}

collection!(
    /// The stacks of a host, or Aegis's own stacks, in name order.
    Stacks,
    Stack
);

/// A host and the stacks assigned to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    name: HostName,
    stacks: Stacks,
}

impl Host {
    /// The host `name` with its `stacks`.
    #[must_use]
    pub const fn new(name: HostName, stacks: Stacks) -> Self {
        Self { name, stacks }
    }

    /// The host name.
    #[must_use]
    pub const fn name(&self) -> &HostName {
        &self.name
    }

    /// The stacks, in name order.
    #[must_use]
    pub const fn stacks(&self) -> &Stacks {
        &self.stacks
    }
}

collection!(
    /// The hosts of a repository, in name order.
    Hosts,
    Host
);

/// Everything the repository declares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DesiredState {
    hosts: Hosts,
    reserved: Stacks,
}

impl DesiredState {
    /// The desired state of `hosts` and Aegis's own `reserved` stacks.
    #[must_use]
    pub const fn new(hosts: Hosts, reserved: Stacks) -> Self {
        Self { hosts, reserved }
    }

    /// The hosts, in name order.
    #[must_use]
    pub const fn hosts(&self) -> &Hosts {
        &self.hosts
    }

    /// Aegis's own stacks from the reserved `aegis/` directory.
    #[must_use]
    pub const fn reserved(&self) -> &Stacks {
        &self.reserved
    }
}
