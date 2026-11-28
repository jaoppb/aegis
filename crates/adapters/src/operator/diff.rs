//! `aegis diff`: the desired state of a checkout against what the agents last reported.

use std::collections::BTreeSet;

use aegis_api::version_1::{
    ControllerStatus, HostStatus, StackState as Reported, StackStatus, StatusReport, TargetStatus,
};
use aegis_application::{Exposure, KeyUse, Outcome, SidecarSpec};
use aegis_domain::{Host, Stack, StackState, TailnetName, ValueError};

use super::page::{Page, Rendered};
use super::stacks::{is_problem, word};

/// The tailnet name an override is rendered with: the controller alone knows the real one.
const PLACEHOLDER_TAILNET: &str = "tailnet.ts.net";

/// The desired state of the checkout and what the controller knows, side by side.
struct Sides<'sides> {
    outcome: &'sides Outcome,
    status: &'sides ControllerStatus,
    known: BTreeSet<String>,
}

/// What one host has in the checkout and what its agent reported.
struct Located<'located> {
    local: Option<&'located Host>,
    report: &'located StatusReport,
}

/// A stack the checkout declares, where it lives and what its host reported for it.
struct Declared<'declared> {
    directory: String,
    stack: &'declared Stack,
    reported: Option<&'declared StackStatus>,
}

/// Compare the stacks of `outcome`, loaded from a checkout, with the last reports of the hosts in
/// `status`.
///
/// A stack that is not deployed as the checkout says, a stack that failed or is invalid, a host
/// that has not reported or is at another commit than the controller's target and a stack still
/// running that the repository no longer declares are findings. A removed stack (orphaned,
/// purged) is the expected outcome of removing it. The override Aegis renders for each stack that
/// exposes a service is shown for the operator to read, and is not a difference.
#[must_use]
pub fn diff(outcome: &Outcome, status: &ControllerStatus) -> Rendered {
    let mut page = Page::default();
    let sides = Sides {
        outcome,
        status,
        known: known_directories(outcome),
    };
    heading(&mut page, &sides);
    for name in host_names(&sides) {
        host_diff(&mut page, &sides, &name);
    }
    overrides(&mut page, outcome);
    page.finish()
}

fn directory_of(host: &Host, stack: &Stack) -> String {
    format!("hosts/{}/{}", host.name(), stack.name())
}

/// Every directory the checkout declares, loaded or rejected.
fn known_directories(outcome: &Outcome) -> BTreeSet<String> {
    let state = outcome.state();
    let hosted = state.hosts().iter().flat_map(|host| {
        let stacks = host.stacks().iter();
        stacks.map(|stack| directory_of(host, stack))
    });
    let reserved = state
        .reserved()
        .iter()
        .map(|stack| format!("aegis/{}", stack.name()));
    let rejected = outcome
        .rejected()
        .iter()
        .map(|each| each.directory().as_str().to_owned());
    hosted.chain(reserved).chain(rejected).collect()
}

fn heading(page: &mut Page, sides: &Sides<'_>) {
    match sides.status.target() {
        Some(target) => page.line(format!("controller target {}", target.commit())),
        None => page.finding("the controller has no target commit yet"),
    }
    for rejection in sides.outcome.rejected() {
        let directory = rejection.directory();
        page.finding(format!("invalid in this checkout: {directory}"));
    }
}

/// The hosts the checkout declares and the controller knows, by name.
fn host_names(sides: &Sides<'_>) -> BTreeSet<String> {
    let declared = sides.outcome.state().hosts().iter();
    let local = declared.map(|host| host.name().to_string());
    let known = sides.status.hosts().iter();
    local
        .chain(known.map(|host| host.host().to_owned()))
        .collect()
}

fn host_diff(page: &mut Page, sides: &Sides<'_>, name: &str) {
    page.line(format!("host {name}"));
    let mut declared = sides.outcome.state().hosts().iter();
    let local = declared.find(|host| host.name().as_str() == name);
    let remote = sides.status.hosts().iter().find(|host| host.host() == name);
    match remote.and_then(HostStatus::report) {
        Some(report) => reported_diff(page, sides, &Located { local, report }),
        None => silent_host(page, local),
    }
}

/// A host the controller has no report from: a finding when the checkout declares stacks for it.
fn silent_host(page: &mut Page, local: Option<&Host>) {
    let waiting = local.is_some_and(|host| !host.stacks().is_empty());
    page.flag(waiting, "  no report from the agent yet");
}

fn reported_diff(page: &mut Page, sides: &Sides<'_>, located: &Located<'_>) {
    commit_line(page, sides, located.report);
    if let Some(host) = located.local {
        declared(page, host, located.report);
    }
    leftovers(page, &sides.known, located.report);
}

fn commit_line(page: &mut Page, sides: &Sides<'_>, report: &StatusReport) {
    let target = sides.status.target().map(TargetStatus::commit);
    let behind = target.filter(|wanted| *wanted != report.commit());
    match behind {
        Some(wanted) => page.finding(format!(
            "  at {}, the controller wants {wanted}",
            report.commit()
        )),
        None => page.line(format!("  at {}", report.commit())),
    }
}

/// The stacks `host` declares, each against what the agent reported for it.
fn declared(page: &mut Page, host: &Host, report: &StatusReport) {
    for stack in host.stacks() {
        let directory = directory_of(host, stack);
        let reported = report
            .stacks()
            .iter()
            .find(|each| each.directory() == directory);
        compare(
            page,
            &Declared {
                directory,
                stack,
                reported,
            },
        );
    }
}

fn compare(page: &mut Page, declared: &Declared<'_>) {
    let state = declared.stack.policy().state();
    let wording = difference(state, declared.reported);
    let line = format!(
        "  {}  {}",
        declared.directory,
        wording.as_deref().unwrap_or("same")
    );
    page.flag(wording.is_some(), line);
    page.indented(
        "    ",
        declared.reported.map_or(&[][..], StackStatus::details),
    );
}

/// How the host's report differs from what the repository says the stack should be, if it does.
fn difference(wanted: StackState, reported: Option<&StackStatus>) -> Option<String> {
    let state = reported.map(StackStatus::state);
    match (wanted, state) {
        (StackState::Active, Some(Reported::Running))
        | (StackState::Purged, Some(Reported::Purged)) => None,
        (StackState::Active, None) => Some("not deployed".to_owned()),
        (StackState::Purged, None) => Some("purge pending".to_owned()),
        (StackState::Active, Some(other)) => Some(mismatch("running", other)),
        (StackState::Purged, Some(other)) => Some(mismatch("purged", other)),
    }
}

fn mismatch(expected: &str, reported: Reported) -> String {
    format!("expected {expected}, the host reports {}", word(reported))
}

/// The stacks the agent reported that the checkout does not declare.
fn leftovers(page: &mut Page, known: &BTreeSet<String>, report: &StatusReport) {
    let stacks = report.stacks().iter();
    for stack in stacks.filter(|stack| !known.contains(stack.directory())) {
        let state = stack.state();
        let line = format!(
            "  {}  {} on the host, not in this checkout",
            stack.directory(),
            word(state)
        );
        page.flag(state == Reported::Running || is_problem(state), line);
    }
}

/// The override Aegis renders for each active stack that exposes a service.
fn overrides(page: &mut Page, outcome: &Outcome) {
    let hosts = outcome.state().hosts().iter();
    let stacks = hosts.flat_map(|host| host.stacks().iter().map(move |stack| (host, stack)));
    let active = stacks.filter(|(_, stack)| stack.policy().state() == StackState::Active);
    for (host, stack) in active {
        override_of(page, &directory_of(host, stack), stack);
    }
}

fn override_of(page: &mut Page, directory: &str, stack: &Stack) {
    let Some(exposure) = Exposure::of(stack.policy()) else {
        return;
    };
    match rendered(&exposure) {
        Ok(text) => {
            let heading = format!("override of {directory} (the tailnet name is a placeholder):");
            page.line(heading);
            page.indented("  ", text.lines());
        }
        Err(error) => page.finding(format!("{directory}: cannot render the override: {error}")),
    }
}

fn rendered(exposure: &Exposure) -> Result<String, ValueError> {
    let tailnet = TailnetName::new(PLACEHOLDER_TAILNET)?;
    let spec = SidecarSpec::new(exposure, tailnet)?;
    Ok(spec.render(KeyUse::Fresh).compose().as_str().to_owned())
}
