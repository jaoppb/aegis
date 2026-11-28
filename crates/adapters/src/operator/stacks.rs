//! How the stacks an agent reported are listed.

use aegis_api::version_1::{StackState, StackStatus, StatusReport};

use super::page::Page;

/// The word for the state of a stack.
pub(super) const fn word(state: StackState) -> &'static str {
    match state {
        StackState::Running => "running",
        StackState::Failed => "failed",
        StackState::Invalid => "invalid",
        StackState::Orphaned => "orphaned",
        StackState::Purged => "purged",
    }
}

/// Whether the state is a problem: a stack that failed or broke a rule. A stack that was removed
/// (orphaned, purged) is the expected outcome of removing it.
pub(super) const fn is_problem(state: StackState) -> bool {
    matches!(state, StackState::Failed | StackState::Invalid)
}

/// One stack of a report: its directory and state, then what the agent said about it.
pub(super) fn stack_lines(page: &mut Page, stack: &StackStatus) {
    let line = format!("  {}  {}", stack.directory(), word(stack.state()));
    page.flag(is_problem(stack.state()), line);
    page.indented("    ", stack.details());
}

/// The stacks of a report, in the order the agent gave them.
pub(super) fn report_lines(page: &mut Page, report: &StatusReport) {
    for stack in report.stacks() {
        stack_lines(page, stack);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operator::page::Finding;

    fn stack(state: StackState) -> StackStatus {
        StackStatus::new("hosts/h/web", state, vec!["why".to_owned()])
    }

    fn finding_of(state: StackState) -> Finding {
        let mut page = Page::default();
        stack_lines(&mut page, &stack(state));
        page.finish().finding()
    }

    #[test]
    fn a_failed_or_invalid_stack_is_a_finding() {
        assert_eq!(finding_of(StackState::Failed), Finding::Found);
        assert_eq!(finding_of(StackState::Invalid), Finding::Found);
    }

    #[test]
    fn a_running_or_removed_stack_is_not_a_finding() {
        assert_eq!(finding_of(StackState::Running), Finding::Clean);
        assert_eq!(finding_of(StackState::Orphaned), Finding::Clean);
        assert_eq!(finding_of(StackState::Purged), Finding::Clean);
    }

    #[test]
    fn a_stack_is_listed_with_its_state_and_details() {
        let mut page = Page::default();
        stack_lines(&mut page, &stack(StackState::Orphaned));
        assert_eq!(page.finish().text(), "  hosts/h/web  orphaned\n    why\n");
    }
}
