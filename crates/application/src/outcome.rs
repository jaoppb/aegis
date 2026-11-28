//! What loading and validating a repository produces.

use aegis_domain::{DesiredState, Rejections};

/// The stacks that will be deployed and the stacks that were rejected.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    state: DesiredState,
    rejected: Rejections,
}

impl Outcome {
    /// The deployable `state` and the `rejected` stacks.
    #[must_use]
    pub const fn new(state: DesiredState, rejected: Rejections) -> Self {
        Self { state, rejected }
    }

    /// The stacks that passed, grouped as the repository declares them.
    #[must_use]
    pub const fn state(&self) -> &DesiredState {
        &self.state
    }

    /// The stacks that were not deployed, with every violation each one has.
    #[must_use]
    pub const fn rejected(&self) -> &Rejections {
        &self.rejected
    }
}
