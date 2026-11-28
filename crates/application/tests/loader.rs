//! Behaviour of the repository loader through its ports, with in-memory fakes.

use std::collections::BTreeMap;

use aegis_application::{
    DirEntry, EntryKind, EntryName, Outcome, ParseError, Reason, RepoSource, SourceError,
    StackParser, load_desired_state,
};
use aegis_domain::{
    ComposeSummary, DesiredState, Problem, RepoPath, StackKind, StackPolicy, StackState, Stacks,
    TailnetTags, UpdatePolicy, ValueError,
};

struct MemoryRepo {
    files: BTreeMap<String, String>,
}

impl MemoryRepo {
    fn with(files: &[(&str, &str)]) -> Self {
        let files = files
            .iter()
            .map(|(path, text)| ((*path).to_owned(), (*text).to_owned()));
        Self {
            files: files.collect(),
        }
    }
}

fn entry_of(rest: &str) -> (String, EntryKind) {
    rest.split_once('/').map_or_else(
        || (rest.to_owned(), EntryKind::Other),
        |(head, _)| (head.to_owned(), EntryKind::Directory),
    )
}

impl RepoSource for MemoryRepo {
    fn contains(&self, path: &RepoPath) -> Result<bool, SourceError> {
        let prefix = format!("{path}/");
        let is_dir = self.files.keys().any(|key| key.starts_with(&prefix));
        Ok(is_dir || self.files.contains_key(path.as_str()))
    }

    fn list_dir(&self, path: &RepoPath) -> Result<Vec<DirEntry>, SourceError> {
        let prefix = format!("{path}/");
        let entries: BTreeMap<String, EntryKind> = self
            .files
            .keys()
            .filter_map(|key| key.strip_prefix(&prefix))
            .map(entry_of)
            .collect();
        Ok(entries
            .into_iter()
            .map(|(name, kind)| DirEntry::new(EntryName::new(name), kind))
            .collect())
    }

    fn read_file(&self, path: &RepoPath) -> Result<String, SourceError> {
        let missing = || SourceError::new(path.clone(), Reason::new(&"not found"));
        self.files.get(path.as_str()).cloned().ok_or_else(missing)
    }
}

/// Fails on `BROKEN`, reports `unknown` as an unknown key and reads `purged` as a tombstone.
struct FakeParser;

fn fake_policy(text: &str) -> Result<StackPolicy, ParseError> {
    match text {
        "unknown" => Err(ParseError::UnknownKey("bogus".to_owned())),
        "garbage" => Err(ParseError::Invalid("bad toml".to_owned())),
        "purged" => Ok(StackPolicy::new(
            None,
            TailnetTags::default(),
            StackState::Purged,
        )),
        _ => Ok(StackPolicy::default()),
    }
}

fn fake_compose(text: &str) -> Result<ComposeSummary, ParseError> {
    let broken = || ParseError::Invalid("not compose".to_owned());
    text.contains("BROKEN")
        .then(broken)
        .map_or_else(|| Ok(ComposeSummary::default()), Err)
}

impl StackParser for FakeParser {
    fn parse_compose(&self, text: &str) -> Result<ComposeSummary, ParseError> {
        fake_compose(text)
    }

    fn parse_policy(&self, text: &str) -> Result<StackPolicy, ParseError> {
        fake_policy(text)
    }
}

fn load(files: &[(&str, &str)]) -> Outcome {
    load_desired_state(&MemoryRepo::with(files), &FakeParser)
}

/// The state of a repository in which nothing is rejected.
fn loaded(files: &[(&str, &str)]) -> DesiredState {
    let outcome = load(files);
    assert!(outcome.rejected().is_empty(), "{:?}", outcome.rejected());
    outcome.state().clone()
}

/// Every rejected stack directory with the problems found in it.
fn problems(outcome: &Outcome) -> Vec<(String, Vec<Problem>)> {
    let rejected = outcome.rejected().iter();
    rejected
        .map(|rejection| {
            let found = rejection.violations().iter();
            let found = found.map(|violation| violation.problem().clone());
            (rejection.directory().to_string(), found.collect())
        })
        .collect()
}

fn stack_names(stacks: &Stacks) -> Vec<String> {
    stacks
        .iter()
        .map(|stack| stack.name().to_string())
        .collect()
}

fn layout(state: &DesiredState) -> Vec<(String, Vec<String>)> {
    let hosts = state.hosts().iter();
    hosts
        .map(|host| (host.name().to_string(), stack_names(host.stacks())))
        .collect()
}

#[test]
fn an_empty_repository_has_no_hosts() {
    assert_eq!(load(&[]), Outcome::default());
}

#[test]
fn hosts_and_stacks_are_loaded_in_name_order() {
    let state = loaded(&[
        ("hosts/zeta/web/compose.yaml", "web"),
        ("hosts/alpha/pihole/compose.yaml", "pihole"),
        ("hosts/alpha/ai/compose.yaml", "ai"),
    ]);
    let expected = [
        (
            "alpha".to_owned(),
            vec!["ai".to_owned(), "pihole".to_owned()],
        ),
        ("zeta".to_owned(), vec!["web".to_owned()]),
    ];
    assert_eq!(layout(&state), expected);
}

#[test]
fn the_compose_file_is_kept_verbatim() {
    let state = loaded(&[("hosts/host/stack/compose.yaml", "services: {}\n")]);
    let stack = &state.hosts().as_slice()[0].stacks().as_slice()[0];
    assert_eq!(stack.compose_source().as_str(), "services: {}\n");
    assert_eq!(stack.kind(), StackKind::Host);
}

#[test]
fn a_missing_policy_file_means_defaults() {
    let state = loaded(&[("hosts/host/stack/compose.yaml", "")]);
    let policy = state.hosts().as_slice()[0].stacks().as_slice()[0].policy();
    assert_eq!(policy.update(), UpdatePolicy::Manual);
    assert_eq!(policy.state(), StackState::Active);
}

#[test]
fn the_policy_file_is_read() {
    let files = [
        ("hosts/host/stack/compose.yaml", ""),
        ("hosts/host/stack/aegis.toml", "purged"),
    ];
    let state = loaded(&files);
    let stack = &state.hosts().as_slice()[0].stacks().as_slice()[0];
    assert_eq!(stack.policy().state(), StackState::Purged);
}

#[test]
fn reserved_stacks_are_not_a_host() {
    let state = loaded(&[("aegis/controller/compose.yaml", "")]);
    assert!(state.hosts().is_empty());
    let stack = &state.reserved().as_slice()[0];
    assert_eq!(stack.name().as_str(), "controller");
    assert_eq!(stack.kind(), StackKind::Reserved);
}

#[test]
fn dotfiles_and_loose_files_are_skipped() {
    let state = loaded(&[
        ("hosts/.git/stack/compose.yaml", "BROKEN"),
        ("hosts/README.md", "BROKEN"),
        ("hosts/host/.hidden/compose.yaml", "BROKEN"),
        ("hosts/host/notes.txt", "BROKEN"),
        ("hosts/host/stack/compose.yaml", ""),
    ]);
    assert_eq!(
        layout(&state),
        [("host".to_owned(), vec!["stack".to_owned()])]
    );
}

#[test]
fn an_unknown_policy_key_rejects_only_its_stack() {
    let files = [
        ("hosts/host/stack/compose.yaml", ""),
        ("hosts/host/stack/aegis.toml", "unknown"),
        ("hosts/host/fine/compose.yaml", ""),
    ];
    let outcome = load(&files);
    let expected = vec![(
        "hosts/host/stack".to_owned(),
        vec![Problem::UnknownKey {
            key: "bogus".to_owned(),
        }],
    )];
    assert_eq!(problems(&outcome), expected);
    assert_eq!(
        layout(outcome.state()),
        [("host".to_owned(), vec!["fine".to_owned()])]
    );
}

#[test]
fn the_violation_names_the_policy_file() {
    let files = [
        ("hosts/host/stack/compose.yaml", ""),
        ("hosts/host/stack/aegis.toml", "unknown"),
    ];
    let outcome = load(&files);
    let violation = &outcome.rejected().as_slice()[0].violations().as_slice()[0];
    assert_eq!(
        violation.location().path().as_str(),
        "hosts/host/stack/aegis.toml"
    );
}

const fn every_problem_files() -> [(&'static str, &'static str); 6] {
    [
        ("hosts/host/no-compose/aegis.toml", ""),
        ("hosts/host/bad-compose/compose.yaml", "BROKEN"),
        ("hosts/host/bad-policy/compose.yaml", ""),
        ("hosts/host/bad-policy/aegis.toml", "garbage"),
        ("hosts/host/bad name/compose.yaml", ""),
        ("hosts/fine/stack/compose.yaml", ""),
    ]
}

fn expected_problems() -> Vec<(String, Vec<Problem>)> {
    let invalid = |reason: &str| reason.to_owned();
    vec![
        (
            "hosts/host/bad name".to_owned(),
            vec![Problem::InvalidName {
                reason: ValueError::ForbiddenChar(' '),
            }],
        ),
        (
            "hosts/host/bad-compose".to_owned(),
            vec![Problem::InvalidCompose {
                reason: invalid("not compose"),
            }],
        ),
        (
            "hosts/host/bad-policy".to_owned(),
            vec![Problem::InvalidPolicy {
                reason: invalid("bad toml"),
            }],
        ),
        (
            "hosts/host/no-compose".to_owned(),
            vec![Problem::MissingCompose],
        ),
    ]
}

#[test]
fn every_problem_is_collected_in_directory_order() {
    let outcome = load(&every_problem_files());
    assert_eq!(problems(&outcome), expected_problems());
    assert_eq!(
        layout(outcome.state()),
        [
            ("fine".to_owned(), vec!["stack".to_owned()]),
            ("host".to_owned(), Vec::new()),
        ]
    );
}

#[test]
fn a_stack_with_two_problems_reports_both() {
    let files = [
        ("hosts/host/stack/compose.yaml", "BROKEN"),
        ("hosts/host/stack/aegis.toml", "garbage"),
    ];
    let outcome = load(&files);
    assert_eq!(outcome.rejected().len(), 1);
    assert_eq!(outcome.rejected().as_slice()[0].violations().len(), 2);
}

#[test]
fn problems_in_hosts_and_reserved_stacks_are_both_reported() {
    let files = [
        ("hosts/host/stack/aegis.toml", ""),
        ("aegis/stack/aegis.toml", ""),
    ];
    assert_eq!(load(&files).rejected().len(), 2);
}

/// A repository whose `failing` operation errors, to prove the failure is reported, not swallowed.
struct BrokenRepo {
    failing: &'static str,
}

impl BrokenRepo {
    fn fail(&self, operation: &str, path: &RepoPath) -> Result<(), SourceError> {
        let broken = SourceError::new(path.clone(), Reason::new(&operation));
        (operation == self.failing)
            .then_some(broken)
            .map_or(Ok(()), Err)
    }
}

fn host_dir_listing() -> Vec<DirEntry> {
    vec![DirEntry::new(EntryName::new("host"), EntryKind::Directory)]
}

impl RepoSource for BrokenRepo {
    fn contains(&self, path: &RepoPath) -> Result<bool, SourceError> {
        self.fail("contains", path).map(|()| true)
    }

    fn list_dir(&self, path: &RepoPath) -> Result<Vec<DirEntry>, SourceError> {
        self.fail("list_dir", path).map(|()| host_dir_listing())
    }

    fn read_file(&self, path: &RepoPath) -> Result<String, SourceError> {
        self.fail("read_file", path).map(|()| String::new())
    }
}

fn source_errors(failing: &'static str) -> Vec<Problem> {
    let outcome = load_desired_state(&BrokenRepo { failing }, &FakeParser);
    problems(&outcome)
        .into_iter()
        .flat_map(|(_, found)| found)
        .collect()
}

fn unreadable_with(reason: &'static str) -> bool {
    source_errors(reason)
        .iter()
        .any(|problem| matches!(problem, Problem::Unreadable { reason: found } if found.contains(reason)))
}

#[test]
fn a_failing_contains_is_reported() {
    assert!(unreadable_with("contains"));
}

#[test]
fn a_failing_list_dir_is_reported() {
    assert!(unreadable_with("list_dir"));
}

#[test]
fn a_failing_read_file_is_reported() {
    assert!(unreadable_with("read_file"));
}

#[test]
fn an_invalid_host_name_is_a_typed_violation() {
    let outcome = load(&[("hosts/bad host/stack/compose.yaml", "")]);
    let expected = vec![(
        "hosts/bad host".to_owned(),
        vec![Problem::InvalidName {
            reason: ValueError::ForbiddenChar(' '),
        }],
    )];
    assert_eq!(problems(&outcome), expected);
}
