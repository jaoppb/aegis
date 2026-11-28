//! Maps a reconcile [`Report`] onto the `/v1` wire types.

use aegis_api::version_1::{StackState, StackStatus, StatusReport};
use aegis_domain::{Deployment, Rejection, Report, StackReport};

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
        Deployment::Orphaned(remains) => {
            let kept = remains.iter().map(|each| each.as_str().to_owned());
            StackStatus::new(directory, StackState::Orphaned, kept.collect())
        }
        Deployment::Purged => {
            let note = "purged, safe to delete dir".to_owned();
            StackStatus::new(directory, StackState::Purged, vec![note])
        }
        Deployment::Invalid(violations) => {
            let details = violations.iter().map(ToString::to_string).collect();
            StackStatus::new(directory, StackState::Invalid, details)
        }
    }
}

/// A stack the loader or the validation rules rejected, as every agent will report it.
pub(super) fn rejection_status(rejection: &Rejection) -> StackStatus {
    let details = rejection.violations().iter().map(ToString::to_string);
    let directory = rejection.directory().as_str();
    StackStatus::new(directory, StackState::Invalid, details.collect())
}
