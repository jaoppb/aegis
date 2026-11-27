//! Behaviour of the repository loader through its ports, with in-memory fakes.

use std::collections::BTreeMap;

use aegis_application::{
    DirEntry, EntryKind, LoadError, ParseError, RepoSource, SourceError, StackParser,
    load_desired_state,
};
use aegis_domain::{
    ComposeSummary, DesiredState, RepoPath, Stack, StackKind, StackPolicy, StackState,
    UpdatePolicy, ValueError,
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
            .map(|(name, kind)| DirEntry { name, kind })
            .collect())
    }

    fn read_file(&self, path: &RepoPath) -> Result<String, SourceError> {
        let missing = || SourceError {
            path: path.to_string(),
            message: "not found".to_owned(),
        };
        self.files.get(path.as_str()).cloned().ok_or_else(missing)
    }
}

/// Fails on `BROKEN`, reports `unknown` as an unknown key and reads `purged` as a tombstone.
struct FakeParser;

fn fake_policy(text: &str) -> Result<StackPolicy, ParseError> {
    match text {
        "unknown" => Err(ParseError::UnknownKey("bogus".to_owned())),
        "garbage" => Err(ParseError::Invalid("bad toml".to_owned())),
        "purged" => Ok(StackPolicy {
            state: StackState::Purged,
            ..StackPolicy::default()
        }),
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

fn load(files: &[(&str, &str)]) -> Result<DesiredState, Vec<LoadError>> {
    load_desired_state(&MemoryRepo::with(files), &FakeParser)
}

fn stack_names(stacks: &[Stack]) -> Vec<String> {
    stacks.iter().map(|stack| stack.name.to_string()).collect()
}

fn layout(state: &DesiredState) -> Vec<(String, Vec<String>)> {
    let hosts = state.hosts.iter();
    hosts
        .map(|host| (host.name.to_string(), stack_names(&host.stacks)))
        .collect()
}

#[test]
fn an_empty_repository_has_no_hosts() {
    assert_eq!(load(&[]), Ok(DesiredState::default()));
}

#[test]
fn hosts_and_stacks_are_loaded_in_name_order() {
    let state = load(&[
        ("hosts/zeta/web/compose.yaml", "web"),
        ("hosts/alpha/pihole/compose.yaml", "pihole"),
        ("hosts/alpha/ai/compose.yaml", "ai"),
    ])
    .unwrap();
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
    let state = load(&[("hosts/host/stack/compose.yaml", "services: {}\n")]).unwrap();
    let stack = &state.hosts[0].stacks[0];
    assert_eq!(stack.compose_source, "services: {}\n");
    assert_eq!(stack.kind, StackKind::Host);
}

#[test]
fn a_missing_policy_file_means_defaults() {
    let state = load(&[("hosts/host/stack/compose.yaml", "")]).unwrap();
    let policy = &state.hosts[0].stacks[0].policy;
    assert_eq!(policy.update, UpdatePolicy::Manual);
    assert_eq!(policy.state, StackState::Active);
}

#[test]
fn the_policy_file_is_read() {
    let files = [
        ("hosts/host/stack/compose.yaml", ""),
        ("hosts/host/stack/aegis.toml", "purged"),
    ];
    let state = load(&files).unwrap();
    assert_eq!(state.hosts[0].stacks[0].policy.state, StackState::Purged);
}

#[test]
fn reserved_stacks_are_not_a_host() {
    let state = load(&[("aegis/controller/compose.yaml", "")]).unwrap();
    assert!(state.hosts.is_empty());
    assert_eq!(state.reserved[0].name.as_str(), "controller");
    assert_eq!(state.reserved[0].kind, StackKind::Reserved);
}

#[test]
fn dotfiles_and_loose_files_are_skipped() {
    let state = load(&[
        ("hosts/.git/stack/compose.yaml", "BROKEN"),
        ("hosts/README.md", "BROKEN"),
        ("hosts/host/.hidden/compose.yaml", "BROKEN"),
        ("hosts/host/notes.txt", "BROKEN"),
        ("hosts/host/stack/compose.yaml", ""),
    ])
    .unwrap();
    assert_eq!(
        layout(&state),
        [("host".to_owned(), vec!["stack".to_owned()])]
    );
}

#[test]
fn an_unknown_policy_key_is_a_typed_error() {
    let files = [
        ("hosts/host/stack/compose.yaml", ""),
        ("hosts/host/stack/aegis.toml", "unknown"),
    ];
    let expected = LoadError::UnknownKey {
        file: "hosts/host/stack/aegis.toml".to_owned(),
        key: "bogus".to_owned(),
    };
    assert_eq!(load(&files), Err(vec![expected]));
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

#[test]
fn every_problem_is_collected_in_directory_order() {
    let expected = [
        LoadError::InvalidName {
            path: "hosts/host/bad name".to_owned(),
            reason: ValueError::ForbiddenChar(' '),
        },
        LoadError::InvalidCompose {
            file: "hosts/host/bad-compose/compose.yaml".to_owned(),
            reason: "not compose".to_owned(),
        },
        LoadError::InvalidPolicy {
            file: "hosts/host/bad-policy/aegis.toml".to_owned(),
            reason: "bad toml".to_owned(),
        },
        LoadError::MissingCompose {
            file: "hosts/host/no-compose/compose.yaml".to_owned(),
        },
    ];
    assert_eq!(load(&every_problem_files()).unwrap_err(), expected);
}

#[test]
fn a_stack_with_two_problems_reports_both() {
    let files = [
        ("hosts/host/stack/compose.yaml", "BROKEN"),
        ("hosts/host/stack/aegis.toml", "garbage"),
    ];
    assert_eq!(load(&files).unwrap_err().len(), 2);
}

#[test]
fn problems_in_hosts_and_reserved_stacks_are_both_reported() {
    let files = [
        ("hosts/host/stack/aegis.toml", ""),
        ("aegis/stack/aegis.toml", ""),
    ];
    assert_eq!(load(&files).unwrap_err().len(), 2);
}
