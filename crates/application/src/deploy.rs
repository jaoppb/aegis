//! The ports through which a stack is brought up and a checkout is moved to a commit.

use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;

use aegis_domain::{CommitSha, ComposeSummary, HostPath, ProjectName, RepoPath, ServiceName};

use crate::sidecar::Exposure;
use crate::source::Reason;

/// Where a stack is deployed from: its Compose project and its directory in the checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    project: ProjectName,
    directory: RepoPath,
    exposure: Option<Exposure>,
}

impl Target {
    /// The stack in `directory`, run as `project`.
    #[must_use]
    pub const fn new(project: ProjectName, directory: RepoPath) -> Self {
        Self {
            project,
            directory,
            exposure: None,
        }
    }

    /// The same target that exposes a service on the tailnet.
    #[must_use]
    pub fn with_exposure(self, exposure: Option<Exposure>) -> Self {
        Self { exposure, ..self }
    }

    /// What the stack exposes on the tailnet, if anything.
    #[must_use]
    pub const fn exposure(&self) -> Option<&Exposure> {
        self.exposure.as_ref()
    }

    /// The Compose project name.
    #[must_use]
    pub const fn project(&self) -> &ProjectName {
        &self.project
    }

    /// The stack directory, relative to the checkout root.
    #[must_use]
    pub const fn directory(&self) -> &RepoPath {
        &self.directory
    }
}

/// The services that Compose builds from the repository.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuiltServices(Vec<ServiceName>);

impl BuiltServices {
    /// The services, in the order the compose file declares them.
    #[must_use]
    pub const fn new(services: Vec<ServiceName>) -> Self {
        Self(services)
    }

    /// The services.
    #[must_use]
    pub fn as_slice(&self) -> &[ServiceName] {
        &self.0
    }
}

impl FromIterator<ServiceName> for BuiltServices {
    fn from_iter<Source: IntoIterator<Item = ServiceName>>(source: Source) -> Self {
        Self(source.into_iter().collect())
    }
}

/// Absolute paths of extra compose files merged after the stack's own `compose.yaml`. This is the
/// seam for the Aegis override (sidecar, Caddy labels): it is empty until that work lands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtraFiles(Vec<HostPath>);

impl ExtraFiles {
    /// The files, merged in order.
    #[must_use]
    pub const fn new(files: Vec<HostPath>) -> Self {
        Self(files)
    }

    /// The files.
    #[must_use]
    pub fn as_slice(&self) -> &[HostPath] {
        &self.0
    }
}

/// Environment variables set for Compose, such as the secrets SOPS decrypts. This is the seam for
/// secret injection: it is empty until that work lands. Values never reach a log or a `Debug`
/// print.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Environment(BTreeMap<String, String>);

impl Environment {
    /// The variables, by name.
    #[must_use]
    pub const fn new(variables: BTreeMap<String, String>) -> Self {
        Self(variables)
    }

    /// The same variables with `name` set to `value`.
    #[must_use]
    pub fn with_variable(mut self, name: &str, value: &str) -> Self {
        self.0.insert(name.to_owned(), value.to_owned());
        self
    }

    /// The variables, by name.
    #[must_use]
    pub const fn variables(&self) -> &BTreeMap<String, String> {
        &self.0
    }
}

impl fmt::Debug for Environment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Environment({} redacted)", self.0.len())
    }
}

/// What a deploy may add to the stack's own compose file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Extras {
    files: ExtraFiles,
    environment: Environment,
    dotenv: Option<HostPath>,
    sources: ComposeSummary,
}

impl Extras {
    /// Extras made of `files` and `environment`.
    #[must_use]
    pub fn new(files: ExtraFiles, environment: Environment) -> Self {
        Self {
            files,
            environment,
            dotenv: None,
            sources: ComposeSummary::default(),
        }
    }

    /// The same extras that merge `files` after the stack's own compose file.
    #[must_use]
    pub fn with_files(self, files: ExtraFiles) -> Self {
        Self { files, ..self }
    }

    /// The same extras that run Compose with `environment`.
    #[must_use]
    pub fn with_environment(self, environment: Environment) -> Self {
        Self {
            environment,
            ..self
        }
    }

    /// The same extras with the decrypted `.env` at `dotenv`, which Compose reads instead of the
    /// stack's own `.env`.
    #[must_use]
    pub fn with_dotenv(self, dotenv: HostPath) -> Self {
        Self {
            dotenv: Some(dotenv),
            ..self
        }
    }

    /// The same extras that remember the stack's compose `sources`: the env and secret files a
    /// deploy may have to decrypt.
    #[must_use]
    pub fn with_sources(self, sources: ComposeSummary) -> Self {
        Self { sources, ..self }
    }

    /// The decrypted `.env` Compose reads, if the stack has one.
    #[must_use]
    pub const fn dotenv(&self) -> Option<&HostPath> {
        self.dotenv.as_ref()
    }

    /// The stack's compose summary, naming the files to decrypt.
    #[must_use]
    pub const fn sources(&self) -> &ComposeSummary {
        &self.sources
    }

    /// The extra compose files.
    #[must_use]
    pub const fn files(&self) -> &ExtraFiles {
        &self.files
    }

    /// The injected environment.
    #[must_use]
    pub const fn environment(&self) -> &Environment {
        &self.environment
    }
}

/// Everything needed to bring one stack up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeployRequest {
    target: Target,
    commit: CommitSha,
    built: BuiltServices,
    extras: Extras,
}

impl DeployRequest {
    /// A deploy of `target` at `commit` that builds `built`, with no extras.
    #[must_use]
    pub fn new(target: Target, commit: CommitSha, built: BuiltServices) -> Self {
        Self {
            target,
            commit,
            built,
            extras: Extras::default(),
        }
    }

    /// The same deploy with `extras`.
    #[must_use]
    pub fn with_extras(self, extras: Extras) -> Self {
        Self { extras, ..self }
    }

    /// The stack and its project.
    #[must_use]
    pub const fn target(&self) -> &Target {
        &self.target
    }

    /// The commit the images are tagged with.
    #[must_use]
    pub const fn commit(&self) -> &CommitSha {
        &self.commit
    }

    /// The services built from the repository.
    #[must_use]
    pub const fn built(&self) -> &BuiltServices {
        &self.built
    }

    /// The seams for the override and for secrets.
    #[must_use]
    pub const fn extras(&self) -> &Extras {
        &self.extras
    }
}

/// Bringing a stack up failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeError(Reason);

impl ComposeError {
    /// A failure for `reason`.
    #[must_use]
    pub const fn new(reason: Reason) -> Self {
        Self(reason)
    }
}

impl ComposeError {
    /// Why Compose failed.
    #[must_use]
    pub const fn reason(&self) -> &Reason {
        &self.0
    }
}

impl fmt::Display for ComposeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "compose failed: {}", self.0)
    }
}

impl std::error::Error for ComposeError {}

/// Brings stacks up with Docker Compose.
pub trait ComposeRunner {
    /// Converge the stack of `request`. Running it again with nothing changed changes nothing.
    ///
    /// # Errors
    ///
    /// Returns a [`ComposeError`] when Compose cannot be run or reports a failure.
    fn converge(
        &self,
        request: &DeployRequest,
    ) -> impl Future<Output = Result<(), ComposeError>> + Send;
}

/// Moving the checkout to a commit failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitError(Reason);

impl GitError {
    /// A failure for `reason`.
    #[must_use]
    pub const fn new(reason: Reason) -> Self {
        Self(reason)
    }
}

impl fmt::Display for GitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "git failed: {}", self.0)
    }
}

impl std::error::Error for GitError {}

/// A git working tree that can be moved to a commit.
pub trait Checkout {
    /// Fetch what is new from the remote and check `commit` out, detached.
    ///
    /// # Errors
    ///
    /// Returns a [`GitError`] when git cannot be run, times out or reports a failure.
    fn sync(&self, commit: &CommitSha) -> impl Future<Output = Result<(), GitError>> + Send;
}

/// A git working tree that follows the newest commit of a remote branch.
pub trait Upstream {
    /// Fetch the branch, check its newest commit out, detached, and say which commit that is.
    ///
    /// # Errors
    ///
    /// Returns a [`GitError`] when git cannot be run, times out or reports a failure.
    fn advance(&self) -> impl Future<Output = Result<CommitSha, GitError>> + Send;
}
