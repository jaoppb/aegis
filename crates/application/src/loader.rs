//! Turns a repository checkout into the typed desired state.

use aegis_domain::{
    ComposeSource, ComposeSummary, DesiredState, Host, HostName, Hosts, RepoPath, Stack,
    StackDefinition, StackIdentity, StackKind, StackName, StackPolicy, Stacks, ValueError,
};

use crate::error::LoadError;
use crate::parse::{ParseError, StackParser};
use crate::source::{DirEntry, EntryName, RepoSource};

const HOSTS_DIR: &str = "hosts";
const RESERVED_DIR: &str = "aegis";
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

type Problems<T> = Result<T, Vec<LoadError>>;

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
/// A problem in one stack does not stop the others: the result is either the whole desired state
/// or every problem found.
///
/// # Errors
///
/// Returns every [`LoadError`] found, in directory order.
pub fn load_desired_state(
    source: &dyn RepoSource,
    parser: &dyn StackParser,
) -> Result<DesiredState, Vec<LoadError>> {
    let ports = Ports { source, parser };
    let hosts = load_hosts(ports);
    let reserved = load_reserved(ports);
    match (hosts, reserved) {
        (Ok(hosts), Ok(reserved)) => Ok(DesiredState::new(hosts, reserved)),
        (hosts, reserved) => Err(failures(hosts.err(), reserved.err())),
    }
}

fn failures(first: Option<Vec<LoadError>>, second: Option<Vec<LoadError>>) -> Vec<LoadError> {
    first.into_iter().chain(second).flatten().collect()
}

/// Split `results` into the successes, or every failure when there is any.
fn gather<T>(results: impl Iterator<Item = Problems<T>>) -> Problems<Vec<T>> {
    let mut values = Vec::new();
    let mut errors = Vec::new();
    for result in results {
        match result {
            Ok(value) => values.push(value),
            Err(found) => errors.extend(found),
        }
    }
    if errors.is_empty() {
        Ok(values)
    } else {
        Err(errors)
    }
}

fn single<T>(result: Result<T, LoadError>) -> Problems<T> {
    result.map_err(|error| vec![error])
}

fn root(name: &str) -> Result<RepoPath, LoadError> {
    RepoPath::new(name).map_err(|reason| LoadError::InvalidName {
        path: name.to_owned(),
        reason,
    })
}

/// Validate directory `name` with `make` and join it onto `parent`.
fn named<T>(
    parent: &RepoPath,
    name: &str,
    make: fn(&str) -> Result<T, ValueError>,
) -> Result<Named<T>, LoadError> {
    let joined = make(name).and_then(|value| {
        let path = parent.join(name)?;
        Ok(Named { value, path })
    });
    joined.map_err(|reason| LoadError::InvalidName {
        path: format!("{parent}/{name}"),
        reason,
    })
}

/// The visible subdirectories of `dir`, sorted. A missing `dir` has none.
fn subdirectories(ports: Ports<'_>, dir: &RepoPath) -> Result<Vec<EntryName>, LoadError> {
    if !ports.source.contains(dir).map_err(LoadError::Source)? {
        return Ok(Vec::new());
    }
    let entries = ports.source.list_dir(dir).map_err(LoadError::Source)?;
    let mut names: Vec<EntryName> = entries
        .into_iter()
        .filter(DirEntry::is_visible_directory)
        .map(|entry| entry.name().clone())
        .collect();
    names.sort();
    Ok(names)
}

fn load_hosts(ports: Ports<'_>) -> Problems<Hosts> {
    let hosts_dir = single(root(HOSTS_DIR))?;
    let names = single(subdirectories(ports, &hosts_dir))?;
    let hosts = gather(names.iter().map(|name| load_host(ports, &hosts_dir, name)))?;
    Ok(Hosts::new(hosts))
}

fn load_host(ports: Ports<'_>, hosts_dir: &RepoPath, name: &EntryName) -> Problems<Host> {
    let host = single(named(hosts_dir, name.as_str(), |raw| HostName::new(raw)))?;
    let scope = Scope {
        dir: host.path,
        kind: StackKind::Host,
    };
    Ok(Host::new(host.value, load_stacks(ports, &scope)?))
}

fn load_reserved(ports: Ports<'_>) -> Problems<Stacks> {
    let dir = single(root(RESERVED_DIR))?;
    load_stacks(
        ports,
        &Scope {
            dir,
            kind: StackKind::Reserved,
        },
    )
}

fn load_stacks(ports: Ports<'_>, scope: &Scope) -> Problems<Stacks> {
    let names = single(subdirectories(ports, &scope.dir))?;
    let stacks = gather(names.iter().map(|name| load_stack(ports, scope, name)))?;
    Ok(Stacks::new(stacks))
}

fn load_stack(ports: Ports<'_>, scope: &Scope, name: &EntryName) -> Problems<Stack> {
    let stack = single(named(&scope.dir, name.as_str(), |raw| StackName::new(raw)))?;
    let compose = read_compose(ports, &stack.path);
    let policy = read_policy(ports, &stack.path);
    match (compose, policy) {
        (Ok(compose), Ok(policy)) => {
            let identity = StackIdentity::new(stack.value, scope.kind);
            let definition = StackDefinition::new(policy, compose.source, compose.summary);
            Ok(Stack::new(identity, definition))
        }
        (compose, policy) => Err(failures(compose.err(), policy.err())),
    }
}

/// The path and text of `file` in `dir`, or `None` when the file is absent.
fn read_optional(ports: Ports<'_>, dir: &RepoPath, file: &str) -> Problems<Option<Located>> {
    let path = single(dir.join(file).map_err(|reason| invalid_path(dir, reason)))?;
    if !single(ports.source.contains(&path).map_err(LoadError::Source))? {
        return Ok(None);
    }
    let text = single(ports.source.read_file(&path).map_err(LoadError::Source))?;
    Ok(Some(Located {
        file: path,
        text: FileText(text),
    }))
}

fn read_compose(ports: Ports<'_>, dir: &RepoPath) -> Problems<ComposeFile> {
    let Some(located) = read_optional(ports, dir, COMPOSE_FILE)? else {
        return Err(vec![LoadError::MissingCompose {
            file: format!("{dir}/{COMPOSE_FILE}"),
        }]);
    };
    let parsed = ports.parser.parse_compose(located.text.as_str());
    let summary = parsed.map_err(|error| vec![compose_error(&located.file, &error)])?;
    Ok(ComposeFile {
        source: ComposeSource::new(located.text.into_string()),
        summary,
    })
}

fn read_policy(ports: Ports<'_>, dir: &RepoPath) -> Problems<StackPolicy> {
    match read_optional(ports, dir, POLICY_FILE)? {
        None => Ok(StackPolicy::default()),
        Some(located) => parse_policy(ports, &located.file, located.text.as_str()),
    }
}

fn parse_policy(ports: Ports<'_>, file: &RepoPath, text: &str) -> Problems<StackPolicy> {
    ports
        .parser
        .parse_policy(text)
        .map_err(|error| vec![policy_error(file, error)])
}

fn invalid_path(dir: &RepoPath, reason: ValueError) -> LoadError {
    LoadError::InvalidName {
        path: dir.to_string(),
        reason,
    }
}

fn compose_error(file: &RepoPath, error: &ParseError) -> LoadError {
    LoadError::InvalidCompose {
        file: file.to_string(),
        reason: error.to_string(),
    }
}

fn policy_error(file: &RepoPath, error: ParseError) -> LoadError {
    let file = file.to_string();
    match error {
        ParseError::UnknownKey(key) => LoadError::UnknownKey { file, key },
        ParseError::Invalid(reason) => LoadError::InvalidPolicy { file, reason },
    }
}
