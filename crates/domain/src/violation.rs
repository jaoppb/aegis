//! What is wrong with a stack: the problems the loader and the validation rules report.

use std::fmt;

use crate::value::{DeclaredPath, Line, RepoPath, ServiceName, ValueError, VolumeName, collection};

/// A file, and the line in it when it is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    file: RepoPath,
    line: Option<Line>,
}

impl Location {
    /// The whole of `file`.
    #[must_use]
    pub const fn file(file: RepoPath) -> Self {
        Self { file, line: None }
    }

    /// The same location at `line`, when it is known.
    #[must_use]
    pub fn at(self, line: Option<Line>) -> Self {
        Self { line, ..self }
    }

    /// The file.
    #[must_use]
    pub const fn path(&self) -> &RepoPath {
        &self.file
    }

    /// The line, when it is known.
    #[must_use]
    pub const fn line(&self) -> Option<Line> {
        self.line
    }
}

impl fmt::Display for Location {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(formatter, "{}:{line}", self.file),
            None => write!(formatter, "{}", self.file),
        }
    }
}

/// What kind of file a missing reference was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileRole {
    /// An `env_file` entry.
    EnvFile,
    /// The file of a top-level secret.
    Secret,
    /// An `include`, `extends` or `configs` file.
    Referenced,
}

impl fmt::Display for FileRole {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::EnvFile => "env file",
            Self::Secret => "secret file",
            Self::Referenced => "referenced file",
        })
    }
}

/// One thing wrong with a stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// The repository could not be read.
    Unreadable {
        /// What the reader reported.
        reason: String,
    },
    /// A directory name is not a valid host or stack name.
    InvalidName {
        /// Why the name was rejected.
        reason: ValueError,
    },
    /// The stack directory has no `compose.yaml`.
    MissingCompose,
    /// The `compose.yaml` could not be understood.
    InvalidCompose {
        /// What was wrong.
        reason: String,
    },
    /// The `aegis.toml` is malformed or holds an invalid value.
    InvalidPolicy {
        /// What was wrong.
        reason: String,
    },
    /// The `aegis.toml` has a key outside the schema.
    UnknownKey {
        /// The unknown key.
        key: String,
    },
    /// A service mounts a named volume the compose file never declares.
    UndeclaredVolume {
        /// The service.
        service: ServiceName,
        /// The volume.
        volume: VolumeName,
    },
    /// A relative bind mount is writable.
    WritableRelativeMount {
        /// The service.
        service: ServiceName,
        /// The mounted path.
        source: RepoPath,
    },
    /// A relative bind mount points at a path the repository does not have.
    MissingMountSource {
        /// The service.
        service: ServiceName,
        /// The mounted path.
        source: RepoPath,
    },
    /// A build context or Dockerfile is outside the stack directory.
    BuildOutsideStack {
        /// The service.
        service: ServiceName,
        /// The offending path.
        path: DeclaredPath,
    },
    /// A build context is missing from the repository.
    MissingBuildContext {
        /// The service.
        service: ServiceName,
        /// The context.
        context: RepoPath,
    },
    /// A referenced file is missing from the repository.
    MissingFile {
        /// What the file is for.
        role: FileRole,
        /// The file.
        path: RepoPath,
    },
    /// A file is merged into the stack by `include` or `extends`, and its contents are not
    /// validated.
    UnvalidatedInclude {
        /// The file.
        path: RepoPath,
    },
    /// An env or secret file is not SOPS-encrypted.
    NotEncrypted {
        /// The file.
        path: RepoPath,
    },
    /// A service the stack exposes is not in its compose file.
    UnknownExposedService {
        /// The service `expose` names.
        service: ServiceName,
    },
    /// A stack that exposes a service defines a service Aegis injects itself.
    ReservedSidecarService {
        /// The service, `tailscale` or `caddy`.
        service: ServiceName,
    },
    /// The service that joins the tailnet node sets its own network, which would clash with the
    /// node's network namespace.
    OwnNetworkOnNodeService {
        /// The service `node` names.
        service: ServiceName,
    },
    /// A stack that exposes a service declares a volume Aegis injects itself.
    ReservedSidecarVolume {
        /// The volume, which Compose would merge into the injected one.
        volume: VolumeName,
    },
}

impl fmt::Display for Problem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&describe(self))
    }
}

fn describe(problem: &Problem) -> String {
    match problem {
        Problem::Unreadable { reason } => format!("cannot read the repository: {reason}"),
        Problem::InvalidName { reason } => format!("invalid name: {reason}"),
        Problem::MissingCompose => "the stack has no compose.yaml".to_owned(),
        Problem::InvalidCompose { reason } => format!("invalid compose file: {reason}"),
        Problem::InvalidPolicy { reason } => format!("invalid aegis.toml: {reason}"),
        Problem::UnknownKey { key } => format!("aegis.toml has an unknown key `{key}`"),
        other => describe_rule(other),
    }
}

fn describe_rule(problem: &Problem) -> String {
    match problem {
        Problem::UndeclaredVolume { service, volume } => format!(
            "service `{service}` mounts volume `{volume}`, which is not declared under top-level `volumes`"
        ),
        Problem::WritableRelativeMount { service, source } => format!(
            "service `{service}` mounts `{source}` writable: a relative bind mount must be `:ro`; keep data in a named volume or an absolute host path"
        ),
        Problem::MissingMountSource { service, source } => format!(
            "service `{service}` mounts `{source}`, which is not in the repository: commit it or use a named volume or an absolute host path"
        ),
        other => describe_file(other),
    }
}

fn describe_file(problem: &Problem) -> String {
    match problem {
        Problem::BuildOutsideStack { service, path } => format!(
            "service `{service}` builds from `{path}`, which is outside the stack directory"
        ),
        Problem::MissingBuildContext { service, context } => {
            format!("service `{service}` builds from `{context}`, which is not in the repository")
        }
        Problem::MissingFile { role, path } => format!("{role} `{path}` is not in the repository"),
        Problem::UnvalidatedInclude { path } => format!(
            "`{path}` is merged in by include or extends, and Aegis does not validate included files yet: inline it into compose.yaml"
        ),
        Problem::NotEncrypted { path } => format!(
            "`{path}` is not SOPS-encrypted: encrypt it with `sops --encrypt` before committing"
        ),
        other => describe_exposure(other),
    }
}

fn describe_exposure(problem: &Problem) -> String {
    match problem {
        Problem::UnknownExposedService { service } => {
            format!("aegis.toml exposes service `{service}`, which compose.yaml does not define")
        }
        Problem::ReservedSidecarService { service } => format!(
            "service `{service}` is injected by Aegis when the stack exposes a service or sets a node: rename it or remove the exposure"
        ),
        Problem::OwnNetworkOnNodeService { service } => format!(
            "service `{service}` joins the tailnet node's network, so it may not set `network_mode`, `networks`, `ports` or `hostname`: remove them"
        ),
        Problem::ReservedSidecarVolume { volume } => format!(
            "volume `{volume}` is injected by Aegis when the stack exposes a service or sets a node: rename it or remove the exposure"
        ),
        other => format!("{other:?}"),
    }
}

/// A [`Problem`] and where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    location: Location,
    problem: Problem,
}

impl Violation {
    /// `problem` found at `location`.
    #[must_use]
    pub const fn new(location: Location, problem: Problem) -> Self {
        Self { location, problem }
    }

    /// Where the problem is.
    #[must_use]
    pub const fn location(&self) -> &Location {
        &self.location
    }

    /// What is wrong.
    #[must_use]
    pub const fn problem(&self) -> &Problem {
        &self.problem
    }
}

impl fmt::Display for Violation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.location, self.problem)
    }
}

collection!(
    /// The violations of one stack, in the order they were found.
    Violations,
    Violation
);

/// A stack that is not deployed, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    directory: RepoPath,
    violations: Violations,
}

impl Rejection {
    /// The stack in `directory` is rejected for `violations`.
    #[must_use]
    pub const fn new(directory: RepoPath, violations: Violations) -> Self {
        Self {
            directory,
            violations,
        }
    }

    /// The stack directory, such as `hosts/jao-server/pihole`.
    #[must_use]
    pub const fn directory(&self) -> &RepoPath {
        &self.directory
    }

    /// Everything wrong with the stack.
    #[must_use]
    pub const fn violations(&self) -> &Violations {
        &self.violations
    }
}

collection!(
    /// The stacks that are not deployed: those the loader rejected, then those validation rejected.
    Rejections,
    Rejection
);

#[cfg(test)]
mod tests {
    use super::*;

    fn text(reason: &str) -> String {
        reason.to_owned()
    }

    fn load_problems() -> Vec<Problem> {
        vec![
            Problem::Unreadable { reason: text("r") },
            Problem::InvalidName {
                reason: ValueError::Empty,
            },
            Problem::MissingCompose,
            Problem::InvalidCompose { reason: text("r") },
            Problem::InvalidPolicy { reason: text("r") },
            Problem::UnknownKey { key: text("k") },
        ]
    }

    fn rule_problems() -> Vec<Problem> {
        let path = || RepoPath::new("a").unwrap();
        let service = || ServiceName::new("s").unwrap();
        vec![
            Problem::UndeclaredVolume {
                service: service(),
                volume: VolumeName::new("v").unwrap(),
            },
            Problem::WritableRelativeMount {
                service: service(),
                source: path(),
            },
            Problem::MissingMountSource {
                service: service(),
                source: path(),
            },
            Problem::MissingFile {
                role: FileRole::EnvFile,
                path: path(),
            },
            Problem::NotEncrypted { path: path() },
            Problem::UnvalidatedInclude { path: path() },
        ]
    }

    fn exposure_problems() -> Vec<Problem> {
        let service = || ServiceName::new("s").unwrap();
        vec![
            Problem::UnknownExposedService { service: service() },
            Problem::ReservedSidecarService { service: service() },
            Problem::OwnNetworkOnNodeService { service: service() },
            Problem::ReservedSidecarVolume {
                volume: VolumeName::new("v").unwrap(),
            },
        ]
    }

    fn build_problems() -> Vec<Problem> {
        let service = || ServiceName::new("s").unwrap();
        vec![
            Problem::BuildOutsideStack {
                service: service(),
                path: DeclaredPath::new("a").unwrap(),
            },
            Problem::MissingBuildContext {
                service: service(),
                context: RepoPath::new("a").unwrap(),
            },
        ]
    }

    fn described(problem: &Problem) -> bool {
        let message = problem.to_string();
        !message.is_empty() && !message.contains('{')
    }

    #[test]
    fn every_problem_has_a_message() {
        let all = [
            load_problems(),
            rule_problems(),
            build_problems(),
            exposure_problems(),
        ]
        .concat();
        assert!(all.iter().all(described), "{all:?}");
    }
}
