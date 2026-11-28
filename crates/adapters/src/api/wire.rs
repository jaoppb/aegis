//! Maps a reconcile [`Report`] onto the `/v1` wire types.

use aegis_api::version_1::{StackState, StackStatus, StatusReport};
use aegis_domain::{Deployment, Report, StackReport};

pub(super) fn status_report(report: &Report) -> StatusReport {
    let origin = report.origin();
    let stacks = report.stacks().iter().map(stack_status).collect();
    StatusReport::new(origin.host().as_str(), origin.commit().as_str(), stacks)
}

fn stack_status(report: &StackReport) -> StackStatus {
    let directory = report.directory().as_str();
    match report.deployment() {
        Deployment::Running => StackStatus::new(directory, StackState::Running, Vec::new()),
        Deployment::Failed(detail) => StackStatus::new(
            directory,
            StackState::Failed,
            vec![detail.as_str().to_owned()],
        ),
        Deployment::Invalid(violations) => {
            let details = violations.iter().map(ToString::to_string).collect();
            StackStatus::new(directory, StackState::Invalid, details)
        }
    }
}
