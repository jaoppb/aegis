//! `aegis validate`: what a checkout of the repository is made of and what is wrong with it.

use aegis_application::Outcome;
use aegis_domain::Rejection;

use super::page::{Page, Rendered};

/// The stacks that passed and the ones that did not, each rejected one with its violations.
#[must_use]
pub fn validate(outcome: &Outcome) -> Rendered {
    let mut page = Page::default();
    for rejection in outcome.rejected() {
        rejected(&mut page, rejection);
    }
    let valid = valid_stacks(outcome);
    let broken = outcome.rejected().len();
    if broken == 0 {
        page.line(format!("ok: {valid} stacks"));
    } else {
        page.line(format!("{broken} rejected, {valid} valid"));
    }
    page.finish()
}

fn rejected(page: &mut Page, rejection: &Rejection) {
    page.finding(format!("invalid {}", rejection.directory()));
    let problems = rejection.violations().iter();
    page.indented("  ", problems.map(ToString::to_string));
}

fn valid_stacks(outcome: &Outcome) -> usize {
    let state = outcome.state();
    let hosted = state.hosts().iter().flat_map(|host| host.stacks().iter());
    hosted.chain(state.reserved().iter()).count()
}

#[cfg(test)]
mod tests {
    use aegis_domain::{DesiredState, Rejections, RepoPath, Violations};

    use super::*;
    use crate::operator::page::Finding;

    #[test]
    fn a_clean_checkout_is_ok_and_counts_its_stacks() {
        let done = validate(&Outcome::default());
        assert_eq!(done.text(), "ok: 0 stacks\n");
        assert_eq!(done.finding(), Finding::Clean);
    }

    #[test]
    fn a_rejected_stack_is_named_and_is_a_finding() {
        let directory = RepoPath::new("hosts/h/broken").unwrap();
        let rejected = Rejections::new(vec![Rejection::new(directory, Violations::default())]);
        let outcome = Outcome::new(DesiredState::default(), rejected);
        let done = validate(&outcome);
        assert_eq!(done.text(), "invalid hosts/h/broken\n1 rejected, 0 valid\n");
        assert_eq!(done.finding(), Finding::Found);
    }
}
