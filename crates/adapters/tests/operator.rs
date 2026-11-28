//! The operator commands over a real checkout on disk: what `validate` and `diff` say about it.

use std::path::{Path, PathBuf};

use aegis_adapters::operator::{Finding, diff, load_checkout, validate};
use aegis_api::version_1::{
    Agent, Confirmation, ControllerStatus, HostStatus, StackState, StackStatus, StatusReport,
    TargetStatus,
};

#[cfg(test)]
const COMPOSE: &str = "services:\n  web:\n    image: nginx:1.27\n";

#[cfg(test)]
fn checkout(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!("aegis-op-{}-{name}", std::process::id()));
    std::fs::remove_dir_all(&root).ok();
    for (path, text) in files {
        write(&root, path, text);
    }
    root
}

#[cfg(test)]
fn write(root: &Path, path: &str, text: &str) {
    let target = root.join(path);
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(target, text).unwrap();
}

#[cfg(test)]
fn stack(directory: &str, state: StackState) -> StackStatus {
    StackStatus::new(directory, state, Vec::new())
}

#[cfg(test)]
fn status_with(commit: &str, stacks: Vec<StackStatus>) -> ControllerStatus {
    let report = StatusReport::new("h", commit, stacks);
    let host = HostStatus::new("h", Agent::Mapped, Confirmation::Confirmed).with_report(report);
    ControllerStatus::new(Some(TargetStatus::new("abc1234", Vec::new())), vec![host])
}

#[cfg(test)]
const WEB: (&str, &str) = ("hosts/h/web/compose.yaml", COMPOSE);

#[test]
fn a_valid_checkout_validates_clean() {
    let root = checkout("valid", &[WEB]);
    let done = validate(&load_checkout(&root).unwrap());
    assert_eq!(done.finding(), Finding::Clean);
    assert_eq!(done.text(), "ok: 1 stacks\n");
}

#[test]
fn a_stack_that_breaks_a_rule_is_a_finding_with_its_violations() {
    let root = checkout(
        "invalid",
        &[WEB, ("hosts/h/broken/readme", "no compose file")],
    );
    let done = validate(&load_checkout(&root).unwrap());
    assert_eq!(done.finding(), Finding::Found);
    assert!(done.text().contains("invalid hosts/h/broken"));
    assert!(done.text().ends_with("1 rejected, 1 valid\n"));
}

#[test]
fn the_compose_allowlist_applies_offline() {
    let tagged = "services:\n  web:\n    image: !reset nginx\n";
    let root = checkout("allowlist", &[("hosts/h/web/compose.yaml", tagged)]);
    let done = validate(&load_checkout(&root).unwrap());
    assert_eq!(done.finding(), Finding::Found);
}

#[test]
fn a_running_stack_that_the_checkout_declares_is_no_difference() {
    let root = checkout("same", &[WEB]);
    let status = status_with("abc1234", vec![stack("hosts/h/web", StackState::Running)]);
    let done = diff(&load_checkout(&root).unwrap(), &status);
    assert_eq!(done.finding(), Finding::Clean);
    assert!(done.text().contains("hosts/h/web  same"));
}

#[test]
fn a_stack_the_host_does_not_run_is_a_difference() {
    let root = checkout("missing", &[WEB]);
    let done = diff(
        &load_checkout(&root).unwrap(),
        &status_with("abc1234", Vec::new()),
    );
    assert_eq!(done.finding(), Finding::Found);
    assert!(done.text().contains("hosts/h/web  not deployed"));
}

#[test]
fn a_failed_stack_is_a_difference() {
    let root = checkout("failed", &[WEB]);
    let status = status_with("abc1234", vec![stack("hosts/h/web", StackState::Failed)]);
    let done = diff(&load_checkout(&root).unwrap(), &status);
    assert_eq!(done.finding(), Finding::Found);
    assert!(
        done.text()
            .contains("expected running, the host reports failed")
    );
}

#[test]
fn a_stack_removed_from_the_repository_is_expected_to_be_orphaned_or_purged() {
    let root = checkout("removed", &[WEB]);
    let reported = vec![
        stack("hosts/h/web", StackState::Running),
        stack("hosts/h/old", StackState::Orphaned),
        stack("hosts/h/gone", StackState::Purged),
    ];
    let done = diff(
        &load_checkout(&root).unwrap(),
        &status_with("abc1234", reported),
    );
    assert_eq!(done.finding(), Finding::Clean);
    assert!(
        done.text()
            .contains("hosts/h/old  orphaned on the host, not in this checkout")
    );
}

#[test]
fn a_stack_still_running_that_the_checkout_dropped_is_a_difference() {
    let root = checkout("dropped", &[WEB]);
    let reported = vec![
        stack("hosts/h/web", StackState::Running),
        stack("hosts/h/old", StackState::Running),
    ];
    let done = diff(
        &load_checkout(&root).unwrap(),
        &status_with("abc1234", reported),
    );
    assert_eq!(done.finding(), Finding::Found);
}

#[test]
fn a_purge_that_has_not_run_yet_is_a_difference_and_a_finished_one_is_not() {
    let files = [
        WEB,
        ("hosts/h/gone/compose.yaml", COMPOSE),
        ("hosts/h/gone/aegis.toml", "state = \"purged\"\n"),
    ];
    let root = checkout("purge", &files);
    let outcome = load_checkout(&root).unwrap();
    let pending = status_with("abc1234", vec![stack("hosts/h/web", StackState::Running)]);
    assert_eq!(diff(&outcome, &pending).finding(), Finding::Found);
    let finished = vec![
        stack("hosts/h/web", StackState::Running),
        stack("hosts/h/gone", StackState::Purged),
    ];
    assert_eq!(
        diff(&outcome, &status_with("abc1234", finished)).finding(),
        Finding::Clean
    );
}

#[test]
fn a_host_at_another_commit_than_the_target_is_a_difference() {
    let root = checkout("stale", &[WEB]);
    let status = status_with("def5678", vec![stack("hosts/h/web", StackState::Running)]);
    let done = diff(&load_checkout(&root).unwrap(), &status);
    assert_eq!(done.finding(), Finding::Found);
    assert!(
        done.text()
            .contains("at def5678, the controller wants abc1234")
    );
}

#[test]
fn a_host_that_has_not_reported_is_a_difference() {
    let root = checkout("silent", &[WEB]);
    let host = HostStatus::new("h", Agent::Mapped, Confirmation::Pending);
    let target = TargetStatus::new("abc1234", Vec::new());
    let status = ControllerStatus::new(Some(target), vec![host]);
    let done = diff(&load_checkout(&root).unwrap(), &status);
    assert_eq!(done.finding(), Finding::Found);
    assert!(done.text().contains("no report from the agent yet"));
}

#[test]
fn a_stack_that_exposes_a_service_shows_its_override_without_a_key() {
    let files = [
        WEB,
        (
            "hosts/h/web/aegis.toml",
            "[expose]\nservice = \"web\"\nport = 80\n",
        ),
    ];
    let root = checkout("override", &files);
    let status = status_with("abc1234", vec![stack("hosts/h/web", StackState::Running)]);
    let done = diff(&load_checkout(&root).unwrap(), &status);
    assert_eq!(done.finding(), Finding::Clean);
    assert!(done.text().contains("override of hosts/h/web"));
    assert!(done.text().contains("TS_AUTHKEY: \"${AEGIS_TS_AUTHKEY}\""));
    assert!(done.text().contains("hostname: web"));
}

#[test]
fn a_stack_that_is_invalid_in_the_checkout_is_a_difference() {
    let root = checkout("local-invalid", &[WEB, ("hosts/h/broken/readme", "x")]);
    let status = status_with("abc1234", vec![stack("hosts/h/web", StackState::Running)]);
    let done = diff(&load_checkout(&root).unwrap(), &status);
    assert_eq!(done.finding(), Finding::Found);
    assert!(
        done.text()
            .contains("invalid in this checkout: hosts/h/broken")
    );
}
