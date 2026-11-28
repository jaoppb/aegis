//! `aegis status`: what the controller wants and what each host reported.

use aegis_api::version_1::{
    Agent, Confirmation, ControllerStatus, HostStatus, RefusalReason, Refused, StackStatus,
};

use super::page::{Page, Rendered};
use super::stacks::report_lines;

/// The status of the fleet as the controller reports it.
///
/// A host that is not confirmed or has no agent, a stack that failed or is invalid, a commit that
/// cannot be read and a remote that cannot be fetched are findings; a removed stack (orphaned,
/// purged) is not.
#[must_use]
pub fn status(status: &ControllerStatus) -> Rendered {
    let mut page = Page::default();
    target(&mut page, status);
    problems(&mut page, status);
    refusals(&mut page, status);
    for host in status.hosts() {
        host_lines(&mut page, host);
    }
    page.finish()
}

fn target(page: &mut Page, status: &ControllerStatus) {
    let Some(target) = status.target() else {
        page.finding("no commit has loaded yet");
        return;
    };
    page.line(format!("target {}", target.commit()));
    for stack in target.rejected() {
        rejected(page, stack);
    }
}

fn rejected(page: &mut Page, stack: &StackStatus) {
    page.finding(format!("  rejected {}", stack.directory()));
    page.indented("    ", stack.details());
}

fn problems(page: &mut Page, status: &ControllerStatus) {
    if let Some(invalid) = status.invalid() {
        page.finding(format!("newest commit {} cannot be read", invalid.commit()));
        page.indented("  ", invalid.details());
    }
    if let Some(reason) = status.fetch_error() {
        page.finding(format!("cannot fetch the repository: {reason}"));
    }
}

/// The requests for a node the controller refused: shown, never a finding, because a refusal says
/// the controller held a line, not that the fleet is off.
fn refusals(page: &mut Page, status: &ControllerStatus) {
    for refused in status.refusals() {
        page.line(refusal_line(refused));
    }
}

fn refusal_line(refused: &Refused) -> String {
    let reason = match refused.reason() {
        RefusalReason::RateLimited => "too many keys",
        RefusalReason::NotPlaced => "not placed on the host by the desired state",
        RefusalReason::WrongTags => "tags are not the stack's",
        RefusalReason::NotPurged => "node delete not allowed by the desired state",
    };
    let stack = refused.stack().unwrap_or("-");
    let times = refused.times();
    format!(
        "refused {} {stack}: {reason}, {} time(s), last {}s ago",
        refused.host(),
        times.count(),
        times.seconds_ago()
    )
}

fn host_lines(page: &mut Page, host: &HostStatus) {
    let agent = match host.agent() {
        Agent::Mapped => "agent mapped",
        Agent::Unmapped => "agent unmapped",
    };
    let header = format!("{}  {agent}  {}", host.host(), confirmation(host));
    let healthy = host.agent() == Agent::Mapped && *host.confirmation() == Confirmation::Confirmed;
    page.flag(!healthy, header);
    if let Some(report) = host.report() {
        page.line(format!("  at {}", report.commit()));
        report_lines(page, report);
    }
}

fn confirmation(host: &HostStatus) -> String {
    match host.confirmation() {
        Confirmation::Confirmed => "confirmed".to_owned(),
        Confirmation::Pending => "pending".to_owned(),
        Confirmation::Failing(reason) => format!("failing: {reason}"),
    }
}

#[cfg(test)]
mod tests {
    use aegis_api::version_1::{Counted, InvalidCommit, StackState, StatusReport, TargetStatus};

    use super::*;
    use crate::operator::page::Finding;

    fn confirmed(host: &str, stacks: Vec<StackStatus>) -> HostStatus {
        let report = StatusReport::new(host, "abc1234", stacks);
        HostStatus::new(host, Agent::Mapped, Confirmation::Confirmed).with_report(report)
    }

    fn running() -> StackStatus {
        StackStatus::new("hosts/h/web", StackState::Running, Vec::new())
    }

    fn healthy() -> ControllerStatus {
        let target = TargetStatus::new("abc1234", Vec::new());
        ControllerStatus::new(Some(target), vec![confirmed("h", vec![running()])])
    }

    #[test]
    fn a_confirmed_host_with_running_stacks_is_clean() {
        let done = status(&healthy());
        assert_eq!(done.finding(), Finding::Clean);
        assert!(done.text().contains("target abc1234"));
        assert!(done.text().contains("h  agent mapped  confirmed"));
        assert!(done.text().contains("hosts/h/web  running"));
    }

    #[test]
    fn orphaned_and_purged_stacks_are_listed_and_are_not_findings() {
        let kept = StackStatus::new("hosts/h/old", StackState::Orphaned, vec!["volume v".into()]);
        let done = StackStatus::new("hosts/h/gone", StackState::Purged, Vec::new());
        let target = TargetStatus::new("abc1234", Vec::new());
        let hosts = vec![confirmed("h", vec![kept, done])];
        let shown = status(&ControllerStatus::new(Some(target), hosts));
        assert_eq!(shown.finding(), Finding::Clean);
        assert!(shown.text().contains("hosts/h/old  orphaned\n    volume v"));
        assert!(shown.text().contains("hosts/h/gone  purged"));
    }

    #[test]
    fn a_failed_stack_is_a_finding() {
        let failed = StackStatus::new("hosts/h/web", StackState::Failed, vec!["boom".into()]);
        let target = TargetStatus::new("abc1234", Vec::new());
        let hosts = vec![confirmed("h", vec![failed])];
        let shown = status(&ControllerStatus::new(Some(target), hosts));
        assert_eq!(shown.finding(), Finding::Found);
    }

    fn finding_for(host: HostStatus) -> Finding {
        let target = Some(TargetStatus::new("abc1234", Vec::new()));
        status(&ControllerStatus::new(target, vec![host])).finding()
    }

    #[test]
    fn a_pending_or_failing_or_unmapped_host_is_a_finding() {
        let pending = HostStatus::new("h", Agent::Mapped, Confirmation::Pending);
        let failing = HostStatus::new("h", Agent::Mapped, Confirmation::Failing("down".into()));
        let unmapped = HostStatus::new("h", Agent::Unmapped, Confirmation::Pending);
        assert_eq!(finding_for(pending), Finding::Found);
        assert_eq!(finding_for(failing), Finding::Found);
        assert_eq!(finding_for(unmapped), Finding::Found);
    }

    #[test]
    fn a_rejected_stack_of_the_target_is_a_finding() {
        let bad = StackStatus::new("hosts/h/bad", StackState::Invalid, vec!["why".into()]);
        let target = TargetStatus::new("abc1234", vec![bad]);
        let shown = status(&ControllerStatus::new(Some(target), Vec::new()));
        assert_eq!(shown.finding(), Finding::Found);
        assert!(shown.text().contains("rejected hosts/h/bad\n    why"));
    }

    #[test]
    fn an_unreadable_commit_and_a_failed_fetch_are_findings() {
        let target = TargetStatus::new("abc1234", Vec::new());
        let invalid = InvalidCommit::new("def5678", vec!["no hosts".into()]);
        let base = || ControllerStatus::new(Some(target.clone()), Vec::new());
        let unreadable = status(&base().with_invalid(invalid));
        assert_eq!(unreadable.finding(), Finding::Found);
        assert!(
            unreadable
                .text()
                .contains("newest commit def5678 cannot be read")
        );
        let offline = status(&base().with_fetch_error("offline"));
        assert_eq!(offline.finding(), Finding::Found);
    }

    #[test]
    fn a_refusal_is_listed_and_is_not_a_finding() {
        let times = Counted::new(2, 30);
        let refused = Refused::new("h", RefusalReason::RateLimited, times).for_stack("vault");
        let shown = status(&healthy().with_refusals(vec![refused]));
        assert!(
            shown
                .text()
                .contains("refused h vault: too many keys, 2 time(s), last 30s ago")
        );
        assert_eq!(shown.finding(), Finding::Clean);
    }

    #[test]
    fn no_target_yet_is_a_finding() {
        let shown = status(&ControllerStatus::new(None, Vec::new()));
        assert_eq!(shown.finding(), Finding::Found);
    }
}
