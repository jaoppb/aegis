//! Turns a repository checkout into the typed desired state.

use aegis_domain::{
    ComposeSummary, DesiredState, Host, HostName, RepoPath, Stack, StackKind, StackName,
    StackPolicy, ValueError,
};

use crate::error::LoadError;
use crate::parse::{ParseError, StackParser};
use crate::source::{EntryKind, RepoSource};

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
        (Ok(hosts), Ok(reserved)) => Ok(DesiredState { hosts, reserved }),
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
) -> Result<(T, RepoPath), LoadError> {
    let joined = make(name).and_then(|value| parent.join(name).map(|path| (value, path)));
    joined.map_err(|reason| LoadError::InvalidName {
        path: format!("{parent}/{name}"),
        reason,
    })
}

/// The visible subdirectories of `dir`, sorted. A missing `dir` has none.
fn subdirectories(ports: Ports<'_>, dir: &RepoPath) -> Result<Vec<String>, LoadError> {
    if !ports.source.contains(dir).map_err(LoadError::Source)? {
        return Ok(Vec::new());
    }
    let entries = ports.source.list_dir(dir).map_err(LoadError::Source)?;
    let mut names: Vec<String> = entries
        .into_iter()
        .filter(|entry| entry.kind == EntryKind::Directory && !entry.name.starts_with('.'))
        .map(|entry| entry.name)
        .collect();
    names.sort();
    Ok(names)
}

fn load_hosts(ports: Ports<'_>) -> Problems<Vec<Host>> {
    let hosts_dir = single(root(HOSTS_DIR))?;
    let names = single(subdirectories(ports, &hosts_dir))?;
    gather(names.iter().map(|name| load_host(ports, &hosts_dir, name)))
}

fn load_host(ports: Ports<'_>, hosts_dir: &RepoPath, name: &str) -> Problems<Host> {
    let (host_name, dir) = single(named(hosts_dir, name, |raw| HostName::new(raw)))?;
    let stacks = load_stacks(
        ports,
        &Scope {
            dir,
            kind: StackKind::Host,
        },
    )?;
    Ok(Host {
        name: host_name,
        stacks,
    })
}

fn load_reserved(ports: Ports<'_>) -> Problems<Vec<Stack>> {
    let dir = single(root(RESERVED_DIR))?;
    load_stacks(
        ports,
        &Scope {
            dir,
            kind: StackKind::Reserved,
        },
    )
}

fn load_stacks(ports: Ports<'_>, scope: &Scope) -> Problems<Vec<Stack>> {
    let names = single(subdirectories(ports, &scope.dir))?;
    gather(names.iter().map(|name| load_stack(ports, scope, name)))
}

fn load_stack(ports: Ports<'_>, scope: &Scope, name: &str) -> Problems<Stack> {
    let (stack_name, dir) = single(named(&scope.dir, name, |raw| StackName::new(raw)))?;
    let compose = read_compose(ports, &dir);
    let policy = read_policy(ports, &dir);
    match (compose, policy) {
        (Ok((compose_source, compose)), Ok(policy)) => Ok(Stack {
            name: stack_name,
            kind: scope.kind,
            policy,
            compose_source,
            compose,
        }),
        (compose, policy) => Err(failures(compose.err(), policy.err())),
    }
}

fn read_compose(ports: Ports<'_>, dir: &RepoPath) -> Problems<(String, ComposeSummary)> {
    let file = single(
        dir.join(COMPOSE_FILE)
            .map_err(|reason| invalid_path(dir, reason)),
    )?;
    if !single(ports.source.contains(&file).map_err(LoadError::Source))? {
        return Err(vec![LoadError::MissingCompose {
            file: file.to_string(),
        }]);
    }
    let text = single(ports.source.read_file(&file).map_err(LoadError::Source))?;
    let parsed = ports.parser.parse_compose(&text);
    let summary = parsed.map_err(|error| vec![compose_error(&file, &error)])?;
    Ok((text, summary))
}

fn read_policy(ports: Ports<'_>, dir: &RepoPath) -> Problems<StackPolicy> {
    let file = single(
        dir.join(POLICY_FILE)
            .map_err(|reason| invalid_path(dir, reason)),
    )?;
    if !single(ports.source.contains(&file).map_err(LoadError::Source))? {
        return Ok(StackPolicy::default());
    }
    let text = single(ports.source.read_file(&file).map_err(LoadError::Source))?;
    ports
        .parser
        .parse_policy(&text)
        .map_err(|error| vec![policy_error(&file, error)])
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
