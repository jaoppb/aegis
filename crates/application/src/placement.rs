//! Whether a request an agent makes for a tailnet node matches what the desired state says its
//! host runs.

use std::fmt;

use aegis_domain::{NodeLabel, Stack, StackState, TailnetTags};

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

/// Check `request` against `stack`, the stack of the calling host named like the one it asked for.
///
/// The stack must exist, be active, expose a service named like the node and carry exactly the
/// tags asked for.
///
/// # Errors
///
/// Returns [`Misplaced::NotPlaced`] when the stack is missing, purged or exposes no such service,
/// and [`Misplaced::WrongTags`] when its tags differ.
pub fn check_enrollment(
    stack: Option<&Stack>,
    request: &EnrollmentRequest,
) -> Result<(), Misplaced> {
    let policy = stack.map(Stack::policy).ok_or(Misplaced::NotPlaced)?;
    let active = policy.state() == StackState::Active;
    let exposed = policy
        .expose()
        .is_some_and(|expose| expose.service().as_str() == request.node().as_str());
    (active && exposed)
        .then_some(())
        .ok_or(Misplaced::NotPlaced)?;
    same_tags(request.tags(), policy.tailnet_tags())
        .then_some(())
        .ok_or(Misplaced::WrongTags)
}

/// Whether `stack` is marked purged and exposed a service named like `node`: the node of a purged
/// stack, which its agent may ask to delete.
#[must_use]
pub fn is_purged_node(stack: &Stack, node: &NodeLabel) -> bool {
    let policy = stack.policy();
    let named = policy
        .expose()
        .is_some_and(|expose| expose.service().as_str() == node.as_str());
    named && policy.state() == StackState::Purged
}
