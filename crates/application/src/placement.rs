//! Whether a request an agent makes for a tailnet node matches what the desired state says its
//! host runs.

use std::fmt;

use aegis_domain::{
    DesiredState, HostName, NodeLabel, Stack, StackName, StackPolicy, StackState, TailnetTags,
};

use crate::enroll::EnrollmentRequest;

/// A request does not match the desired state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Misplaced {
    /// The stack is not an active stack of the host that exposes a service with that hostname.
    NotPlaced,
    /// The tags asked for are not exactly the tags of the stack.
    WrongTags,
}

impl fmt::Display for Misplaced {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotPlaced => formatter.write_str(
                "the desired state does not place an exposed service of that name on this host",
            ),
            Self::WrongTags => formatter.write_str("the tags are not the tags of the stack"),
        }
    }
}

impl std::error::Error for Misplaced {}

fn same_tags(asked: &TailnetTags, declared: &TailnetTags) -> bool {
    let within = |tags: &TailnetTags, other: &TailnetTags| {
        tags.iter()
            .all(|tag| other.iter().any(|candidate| candidate == tag))
    };
    within(asked, declared) && within(declared, asked)
}

fn exposes(policy: &StackPolicy, node: &NodeLabel) -> bool {
    let expose = policy.expose();
    expose.is_some_and(|found| found.service().as_str() == node.as_str())
}

/// The stack of `host` named `stack`, when the desired state has one.
fn stack_of<'state>(
    state: &'state DesiredState,
    host: &HostName,
    stack: &StackName,
) -> Option<&'state Stack> {
    let own = state.hosts().iter().find(|found| found.name() == host)?;
    own.stacks().iter().find(|found| found.name() == stack)
}

/// Whether the desired state has a stack of `host` named `stack`, whatever its state.
#[must_use]
pub fn knows_stack(state: &DesiredState, host: &HostName, stack: &StackName) -> bool {
    stack_of(state, host, stack).is_some()
}

/// Check `request` of the agent of `host` against the desired state.
///
/// The stack must exist under `host`, be active, expose a service named like the node and carry
/// exactly the tags asked for.
///
/// # Errors
///
/// Returns [`Misplaced::NotPlaced`] when the stack is missing, purged or exposes no such service,
/// and [`Misplaced::WrongTags`] when its tags differ.
pub fn check_enrollment(
    state: &DesiredState,
    host: &HostName,
    request: &EnrollmentRequest,
) -> Result<(), Misplaced> {
    let found = stack_of(state, host, request.stack()).ok_or(Misplaced::NotPlaced)?;
    let policy = found.policy();
    let active = policy.state() == StackState::Active;
    (active && exposes(policy, request.node()))
        .then_some(())
        .ok_or(Misplaced::NotPlaced)?;
    same_tags(request.tags(), policy.tailnet_tags())
        .then_some(())
        .ok_or(Misplaced::WrongTags)
}

/// Whether the agent of `host` may delete the node `node`.
///
/// A stack of `host` must be marked purged with an exposed service named like the node, and no
/// active stack of any host may expose the name, because node names are shared by the whole
/// tailnet.
#[must_use]
pub fn purges_node(state: &DesiredState, host: &HostName, node: &NodeLabel) -> bool {
    let own = state.hosts().iter().find(|found| found.name() == host);
    let in_state = |wanted: StackState| {
        move |stack: &Stack| exposes(stack.policy(), node) && stack.policy().state() == wanted
    };
    let purged = own.is_some_and(|found| found.stacks().iter().any(in_state(StackState::Purged)));
    let mut stacks = state.hosts().iter().flat_map(|found| found.stacks().iter());
    purged && !stacks.any(in_state(StackState::Active))
}
