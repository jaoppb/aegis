//! Turns a repository checkout into the typed desired state.

use aegis_domain::{
    ComposeSource, ComposeSummary, DesiredState, Host, HostName, Hosts, Line, Location, Problem,
    Rejection, Rejections, RepoPath, Stack, StackDefinition, StackIdentity, StackKind, StackName,
    StackPolicy, Stacks, ValueError, Violation, Violations,
};

use crate::outcome::Outcome;
use crate::parse::{ParseError, StackParser};
use crate::source::{DirEntry, EntryName, RepoSource, SourceError};

const COMPOSE_FILE: &str = "compose.yaml";
const POLICY_FILE: &str = "aegis.toml";

/// The two ports every loading step needs.
#[derive(Clone, Copy)]
struct Ports<'ports> {
    source: &'ports dyn RepoSource,
    parser: &'ports dyn StackParser,
}

/// A directory of stacks and what kind of stacks it holds.
struct Scope {
    dir: RepoPath,
    kind: StackKind,
}

/// A stack that loads, or the rejection that explains why it does not.
type Attempt<T> = Result<T, Rejection>;

/// What loaded and what was rejected on the way.
struct Partial<T> {
    loaded: T,
    rejected: Rejections,
}

impl<T> Partial<T> {
    fn clean(loaded: T) -> Self {
        Self {
            loaded,
            rejected: Rejections::default(),
        }
    }
}

/// A validated directory name together with the path it was validated under.
struct Named<T> {
    value: T,
    path: RepoPath,
}

/// The text of a file read from the repository.
struct FileText(String);

impl FileText {
    fn as_str(&self) -> &str {
        &self.0
    }

    fn into_string(self) -> String {
        self.0
    }
}

/// A file that exists in the repository, with its text.
struct Located {
    file: RepoPath,
    text: FileText,
}

/// A `compose.yaml` kept as written and summarised.
struct ComposeFile {
    source: ComposeSource,
    summary: ComposeSummary,
}

/// Load `hosts/<host>/<stack>/` and the reserved `aegis/<stack>/` directories.
///
/// A problem in one stack rejects that stack only: the others are loaded.
#[must_use]
pub fn load_desired_state(source: &dyn RepoSource, parser: &dyn StackParser) -> Outcome {
    let ports = Ports { source, parser };
    let hosts = load_hosts(ports);
    let reserved = load_reserved(ports);
    let rejected = hosts.rejected.into_iter().chain(reserved.rejected);
    Outcome::new(
        DesiredState::new(hosts.loaded, reserved.loaded),
        rejected.collect(),
    )
}

fn here(file: &RepoPath, problem: Problem) -> Violation {
    Violation::new(Location::file(file.clone()), problem)
}

pub(crate) fn unreadable(error: &SourceError) -> Violation {
    let reason = error.reason().to_string();
    here(error.path(), Problem::Unreadable { reason })
}

fn rejection(dir: &RepoPath, violations: Vec<Violation>) -> Rejection {
    Rejection::new(dir.clone(), Violations::new(violations))
}

/// Validate directory `name` with `make` and join it onto `parent`.
fn named<T>(
    parent: &RepoPath,
    name: &str,
    make: fn(&str) -> Result<T, ValueError>,
) -> Attempt<Named<T>> {
    let path = parent.listed(name);
    match make(name) {
        Ok(value) => Ok(Named { value, path }),
        Err(reason) => Err(rejection(
            &path,
            vec![here(&path, Problem::InvalidName { reason })],
        )),
    }
}

/// The visible subdirectories of `dir`, sorted. A missing `dir` has none.
fn subdirectories(ports: Ports<'_>, dir: &RepoPath) -> Result<Vec<EntryName>, SourceError> {
    if !ports.source.contains(dir)? {
        return Ok(Vec::new());
    }
    let entries = ports.source.list_dir(dir)?;
    let mut names: Vec<EntryName> = entries
        .into_iter()
        .filter(DirEntry::is_visible_directory)
        .map(|entry| entry.name().clone())
        .collect();
    names.sort();
    Ok(names)
}

fn load_hosts(ports: Ports<'_>) -> Partial<Hosts> {
    let hosts_dir = RepoPath::hosts_root();
    let names = match subdirectories(ports, &hosts_dir) {
        Ok(names) => names,
        Err(error) => return unreadable_scope(&hosts_dir, &error, Hosts::default()),
    };
    let mut hosts = Vec::new();
    let mut rejected = Vec::new();
    for name in &names {
        absorb(
            &mut hosts,
            &mut rejected,
            load_host(ports, &hosts_dir, name),
        );
    }
    Partial {
        loaded: Hosts::new(hosts),
        rejected: Rejections::new(rejected),
    }
}

/// Keep what loaded and what was rejected, in the order the attempts arrive.
fn absorb<T>(loaded: &mut Vec<T>, rejected: &mut Vec<Rejection>, attempt: Attempt<Partial<T>>) {
    match attempt {
        Ok(part) => {
            loaded.push(part.loaded);
            rejected.extend(part.rejected);
        }
        Err(rejection) => rejected.push(rejection),
    }
}

fn unreadable_scope<T>(dir: &RepoPath, error: &SourceError, empty: T) -> Partial<T> {
    Partial {
        loaded: empty,
        rejected: Rejections::new(vec![rejection(dir, vec![unreadable(error)])]),
    }
}

fn load_host(ports: Ports<'_>, hosts_dir: &RepoPath, name: &EntryName) -> Attempt<Partial<Host>> {
    let host = named(hosts_dir, name.as_str(), |raw| HostName::new(raw))?;
    let scope = Scope {
        dir: host.path,
        kind: StackKind::Host,
    };
    let stacks = load_stacks(ports, &scope);
    Ok(Partial {
        loaded: Host::new(host.value, stacks.loaded),
        rejected: stacks.rejected,
    })
}

fn load_reserved(ports: Ports<'_>) -> Partial<Stacks> {
    let scope = Scope {
        dir: RepoPath::reserved_root(),
        kind: StackKind::Reserved,
    };
    load_stacks(ports, &scope)
}

fn load_stacks(ports: Ports<'_>, scope: &Scope) -> Partial<Stacks> {
    let names = match subdirectories(ports, &scope.dir) {
        Ok(names) => names,
        Err(error) => return unreadable_scope(&scope.dir, &error, Stacks::default()),
    };
    let mut stacks = Vec::new();
    let mut rejected = Vec::new();
    for name in &names {
        let attempt = load_stack(ports, scope, name).map(Partial::clean);
        absorb(&mut stacks, &mut rejected, attempt);
    }
    Partial {
        loaded: Stacks::new(stacks),
        rejected: Rejections::new(rejected),
    }
}

fn load_stack(ports: Ports<'_>, scope: &Scope, name: &EntryName) -> Attempt<Stack> {
    let stack = named(&scope.dir, name.as_str(), |raw| StackName::new(raw))?;
    let compose = read_compose(ports, &stack.path);
    let policy = read_policy(ports, &stack.path);
    match (compose, policy) {
        (Ok(compose), Ok(policy)) => {
            let identity = StackIdentity::new(stack.value, scope.kind);
            let definition = StackDefinition::new(policy, compose.source, compose.summary);
            Ok(Stack::new(identity, definition))
        }
        (compose, policy) => {
            let problems = compose.err().into_iter().chain(policy.err()).flatten();
            Err(rejection(&stack.path, problems.collect()))
        }
    }
}

/// The path and text of `file` in `dir`, or `None` when the file is absent.
fn read_optional(
    ports: Ports<'_>,
    dir: &RepoPath,
    file: &str,
) -> Result<Option<Located>, Vec<Violation>> {
    let path = dir.listed(file);
    let unreadable = |error: SourceError| vec![unreadable(&error)];
    if !ports.source.contains(&path).map_err(unreadable)? {
        return Ok(None);
    }
    let text = ports.source.read_file(&path).map_err(unreadable)?;
    Ok(Some(Located {
        file: path,
        text: FileText(text),
    }))
}

fn read_compose(ports: Ports<'_>, dir: &RepoPath) -> Result<ComposeFile, Vec<Violation>> {
    let Some(located) = read_optional(ports, dir, COMPOSE_FILE)? else {
        let file = dir.listed(COMPOSE_FILE);
        return Err(vec![here(&file, Problem::MissingCompose)]);
    };
    let parsed = ports.parser.parse_compose(located.text.as_str());
    let summary = parsed.map_err(|error| vec![compose_violation(&located, &error)])?;
    Ok(ComposeFile {
        source: ComposeSource::new(located.text.into_string()),
        summary,
    })
}

fn read_policy(ports: Ports<'_>, dir: &RepoPath) -> Result<StackPolicy, Vec<Violation>> {
    match read_optional(ports, dir, POLICY_FILE)? {
        None => Ok(StackPolicy::default()),
        Some(located) => ports
            .parser
            .parse_policy(located.text.as_str())
            .map_err(|error| vec![policy_violation(&located, error)]),
    }
}

fn parse_location(located: &Located, error: &ParseError) -> Location {
    let location = Location::file(located.file.clone());
    match error {
        ParseError::InvalidAt { line, .. } => location.at(Some(*line)),
        ParseError::UnknownKey(key) => {
            let last = key.rsplit('.').next().unwrap_or(key);
            location.at(Line::of(located.text.as_str(), last))
        }
        ParseError::Invalid(_) => location,
    }
}

fn compose_violation(located: &Located, error: &ParseError) -> Violation {
    let problem = Problem::InvalidCompose {
        reason: error.to_string(),
    };
    Violation::new(parse_location(located, error), problem)
}

fn policy_violation(located: &Located, error: ParseError) -> Violation {
    let location = parse_location(located, &error);
    let problem = match error {
        ParseError::UnknownKey(key) => Problem::UnknownKey { key },
        ParseError::Invalid(reason) | ParseError::InvalidAt { reason, .. } => {
            Problem::InvalidPolicy { reason }
        }
    };
    Violation::new(location, problem)
}
