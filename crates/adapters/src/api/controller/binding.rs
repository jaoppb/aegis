//! Binds the requests of an agent to the host it serves and to what the desired state says that
//! host runs.

use std::sync::Arc;
use std::time::Instant;

use aegis_api::version_1::RefusalReason;
use aegis_application::{
    Caller, EnrollmentRequest, Exceeded, Goal, Minting, Misplaced, StateVolume, check_enrollment,
    knows_stack, purges_node,
};
use aegis_domain::{DesiredState, HostName, NodeLabel, StackName};
use axum::http::StatusCode;

use super::Shared;
use super::vigil::Who;
use crate::api::refusal::Refusal;

/// The controller's way of binding a request to its caller: shared with the routes of the sidecar
/// enrolment and of the node deletion, which are built beside the [`super::Hub`].
#[derive(Debug, Clone)]
pub struct Binding {
    shared: Arc<Shared>,
}

impl Binding {
    pub(super) const fn new(shared: Arc<Shared>) -> Self {
        Self { shared }
    }

    /// The host the calling agent serves: its name on the tailnet, which must be a host the
    /// controller has an agent for.
    pub(in crate::api) fn host_of(&self, caller: &Caller) -> Result<HostName, Refusal> {
        self.shared.host_of(caller).map_err(not_an_agent)
    }

    /// Let `request` of the agent of `host` through when the desired state places its stack on
    /// the host, and the key limits allow a mint. The refusals are remembered for the status.
    pub(in crate::api) fn admit_enrollment(
        &self,
        host: &HostName,
        request: &EnrollmentRequest,
    ) -> Result<(), Refusal> {
        let stack = request.stack();
        let judged = self.with_goal(|goal| judge(goal, host, request))?;
        let who = Who::new(host, judged.known.as_ref());
        judged
            .checked
            .map_err(|misplaced| self.misplaced(who, misplaced))?;
        match request.state() {
            StateVolume::Empty => self.reserve(host, stack),
            StateVolume::Populated => Ok(()),
        }
    }

    /// Let the agent of `host` delete the node `node` when the desired state marks a stack of
    /// that host purged with an exposed service named like the node, and no other or
    /// rejected stack still holds the name (see [`purges_node`]).
    pub(in crate::api) fn admit_deletion(
        &self,
        host: &HostName,
        node: &NodeLabel,
    ) -> Result<(), Refusal> {
        let purged = self.with_goal(|goal| purges_node(goal.outcome(), host, node))?;
        purged.then_some(()).ok_or_else(|| self.not_purged(host))
    }

    fn with_goal<Answer>(&self, ask: impl FnOnce(&Goal) -> Answer) -> Result<Answer, Refusal> {
        let book = self.shared.book();
        let answer = book.fleet.goal().map(ask).ok_or_else(unloaded);
        drop(book);
        answer
    }

    fn reserve(&self, host: &HostName, stack: &StackName) -> Result<(), Refusal> {
        let who = Who::new(host, Some(stack));
        let minting = Minting::new(host.clone(), stack.clone());
        let reserved = self
            .shared
            .vigil()
            .limiter()
            .reserve(minting, Instant::now());
        reserved.map_err(|exceeded| self.exceeded(who, &exceeded))
    }

    fn exceeded(&self, who: Who<'_>, exceeded: &Exceeded) -> Refusal {
        self.note(who, RefusalReason::RateLimited);
        Refusal::new(StatusCode::TOO_MANY_REQUESTS, exceeded).retry_after(exceeded.retry_after())
    }

    fn misplaced(&self, who: Who<'_>, misplaced: Misplaced) -> Refusal {
        let reason = match misplaced {
            Misplaced::NotPlaced => RefusalReason::NotPlaced,
            Misplaced::WrongTags => RefusalReason::WrongTags,
        };
        self.note(who, reason);
        Refusal::new(StatusCode::FORBIDDEN, &misplaced)
    }

    fn not_purged(&self, host: &HostName) -> Refusal {
        self.note(Who::new(host, None), RefusalReason::NotPurged);
        Refusal::new(
            StatusCode::FORBIDDEN,
            &"the desired state does not let this host delete that node",
        )
    }

    fn note(&self, who: Who<'_>, reason: RefusalReason) {
        self.shared.vigil().refuse(who, reason);
    }
}

/// What the desired state says of a request for a node: the stack when the host has it at all, and
/// whether the request matches it.
struct Judged {
    known: Option<StackName>,
    checked: Result<(), Misplaced>,
}

fn judge(goal: &Goal, host: &HostName, request: &EnrollmentRequest) -> Judged {
    let state = state_of(goal);
    Judged {
        known: knows_stack(state, host, request.stack()).then(|| request.stack().clone()),
        checked: check_enrollment(state, host, request),
    }
}

const fn state_of(goal: &Goal) -> &DesiredState {
    goal.outcome().state()
}

fn not_an_agent(status: StatusCode) -> Refusal {
    let reason = "the caller is not the agent of a host of this controller";
    Refusal::new(status, &reason)
}

fn unloaded() -> Refusal {
    Refusal::new(StatusCode::SERVICE_UNAVAILABLE, &"no commit has loaded yet")
}
