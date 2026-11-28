//! `aegis sync`: what each host did when it was told to converge now.

use aegis_api::version_1::{HostSync, SyncResponse};

use super::page::{Page, Rendered};
use super::stacks::report_lines;

/// What the hosts did. A host that could not be synced and a stack that failed or is invalid are
/// findings; so is a sync that reached no host at all.
#[must_use]
pub fn sync(response: &SyncResponse) -> Rendered {
    let mut page = Page::default();
    if response.hosts().is_empty() {
        page.finding("no host has an agent to sync");
    }
    for host in response.hosts() {
        host_lines(&mut page, host);
    }
    page.finish()
}

fn host_lines(page: &mut Page, host: &HostSync) {
    if let Some(reason) = host.failure() {
        page.finding(format!("{}: not synced: {reason}", host.host()));
    }
    if let Some(report) = host.report() {
        page.line(format!("{}: at {}", host.host(), report.commit()));
        report_lines(page, report);
    }
}

#[cfg(test)]
mod tests {
    use aegis_api::version_1::{StackState, StackStatus, StatusReport};

    use super::*;
    use crate::operator::page::Finding;

    fn done(state: StackState) -> HostSync {
        let stack = StackStatus::new("hosts/h/web", state, Vec::new());
        HostSync::done("h", StatusReport::new("h", "abc1234", vec![stack]))
    }

    #[test]
    fn a_host_whose_stacks_run_is_clean() {
        let shown = sync(&SyncResponse::new(vec![done(StackState::Running)]));
        assert_eq!(shown.finding(), Finding::Clean);
        assert_eq!(shown.text(), "h: at abc1234\n  hosts/h/web  running\n");
    }

    #[test]
    fn a_failed_stack_is_a_finding() {
        let shown = sync(&SyncResponse::new(vec![done(StackState::Failed)]));
        assert_eq!(shown.finding(), Finding::Found);
    }

    #[test]
    fn a_removed_stack_is_not_a_finding() {
        let shown = sync(&SyncResponse::new(vec![done(StackState::Orphaned)]));
        assert_eq!(shown.finding(), Finding::Clean);
    }

    #[test]
    fn a_host_that_could_not_be_synced_is_a_finding_with_its_reason() {
        let failed = HostSync::failed("h", "the agent is unreachable");
        let shown = sync(&SyncResponse::new(vec![failed]));
        assert_eq!(shown.finding(), Finding::Found);
        assert_eq!(shown.text(), "h: not synced: the agent is unreachable\n");
    }

    #[test]
    fn a_sync_that_reached_no_host_is_a_finding() {
        assert_eq!(sync(&SyncResponse::default()).finding(), Finding::Found);
    }
}
