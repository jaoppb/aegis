//! The reconcile use case: move the checkout to a commit, load and validate the host's stacks, and
//! bring each valid one up. A failing stack never blocks the others.

use std::fmt;
use std::future::Future;

use aegis_domain::{
    CommitSha, Deployment, Detail, Host, HostName, Origin, ProjectName, Rejection, RepoPath,
    Report, ReservedStacks, Stack, StackReport, StackReports, StackState, ValueError,
};

use crate::deploy::{
    BuiltServices, Checkout, ComposeRunner, DeployRequest, Extras, GitError, Target,
};
use crate::loader::load_desired_state;
use crate::outcome::Outcome;
use crate::parse::StackParser;
use crate::source::RepoSource;
use crate::validate::{EncryptionCheck, validate_desired_state};

/// The reconcile could not start: the checkout is not at the commit, so nothing is known about the
/// stacks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileError {
    /// The checkout could not be moved to the commit.
    Checkout(GitError),
}

impl fmt::Display for ReconcileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Checkout(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ReconcileError {}

/// The repository checkout as the loader and the validator read it.
pub struct Repository {
    source: Box<dyn RepoSource + Send + Sync>,
    parser: Box<dyn StackParser + Send + Sync>,
    check: Box<dyn EncryptionCheck + Send + Sync>,
}

impl fmt::Debug for Repository {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Repository")
    }
}

impl Repository {
    /// A repository read through `source`, `parser` and `check`.
    #[must_use]
    pub fn new<Source, Parser, Check>(source: Source, parser: Parser, check: Check) -> Self
    where
        Source: RepoSource + Send + Sync + 'static,
        Parser: StackParser + Send + Sync + 'static,
        Check: EncryptionCheck + Send + Sync + 'static,
    {
        Self {
            source: Box::new(source),
            parser: Box::new(parser),
            check: Box::new(check),
        }
    }

    pub(crate) fn load(&self) -> Outcome {
        let loaded = load_desired_state(&*self.source, &*self.parser);
        validate_desired_state(&loaded, &*self.source, &*self.check)
    }
}

/// The ports a reconcile works through.
#[derive(Debug)]
pub struct ReconcilePorts<Git, Docker> {
    git: Git,
    compose: Docker,
    repository: Repository,
}

impl<Git, Docker> ReconcilePorts<Git, Docker> {
    /// The ports: `git` moves the checkout, `compose` runs stacks, `repository` reads them.
    #[must_use]
    pub const fn new(git: Git, compose: Docker, repository: Repository) -> Self {
        Self {
            git,
            compose,
            repository,
        }
    }
}

/// Converges one host to a commit.
pub trait Reconcile {
    /// Move to `commit` and deploy the host's valid stacks.
    ///
    /// # Errors
    ///
    /// Returns a [`ReconcileError`] when the checkout cannot be moved to `commit`.
    fn reconcile(
        &self,
        commit: CommitSha,
    ) -> impl Future<Output = Result<Report, ReconcileError>> + Send;
}

/// Reconciles the stacks of one host.
#[derive(Debug)]
pub struct Reconciler<Git, Docker> {
    host: HostName,
    ports: ReconcilePorts<Git, Docker>,
    reserved: ReservedStacks,
}

impl<Git, Docker> Reconciler<Git, Docker> {
    /// A reconciler for `host` working through `ports`.
    #[must_use]
    pub const fn new(host: HostName, ports: ReconcilePorts<Git, Docker>) -> Self {
        Self {
            host,
            ports,
            reserved: ReservedStacks::new(Vec::new()),
        }
    }

    /// The same reconciler that also deploys the `reserved` stacks of the `aegis/` directory. A
    /// host deploys none of them unless it opts in.
    #[must_use]
    pub fn with_reserved(self, reserved: ReservedStacks) -> Self {
        Self { reserved, ..self }
    }

    const fn scope(&self) -> Scope<'_> {
        Scope {
            host: &self.host,
            reserved: &self.reserved,
        }
    }
}

impl<Git, Docker> Reconcile for Reconciler<Git, Docker>
where
    Git: Checkout + Sync,
    Docker: ComposeRunner + Sync,
{
    async fn reconcile(&self, commit: CommitSha) -> Result<Report, ReconcileError> {
        let git = self.ports.git.sync(&commit);
        git.await.map_err(ReconcileError::Checkout)?;
        let outcome = self.ports.repository.load();
        let units = units(&outcome, &self.scope());
        let mut stacks = deploy_all(&self.ports.compose, &units, &commit).await;
        stacks.extend(invalid(&outcome, &self.scope()));
        let origin = Origin::new(self.host.clone(), commit);
        Ok(Report::new(origin, stacks.into_iter().collect()))
    }
}

/// A stack to deploy and the directory it lives in.
struct Unit<'stack> {
    directory: RepoPath,
    stack: &'stack Stack,
}

/// Which stacks a host deploys: its own, and the reserved ones it opted in to.
struct Scope<'scope> {
    host: &'scope HostName,
    reserved: &'scope ReservedStacks,
}

impl Scope<'_> {
    fn wants_reserved(&self, stack: &Stack) -> bool {
        self.reserved.iter().any(|name| name == stack.name())
    }
}

fn units<'outcome>(outcome: &'outcome Outcome, scope: &Scope<'_>) -> Vec<Unit<'outcome>> {
    let hosts = outcome.state().hosts().iter();
    let own = hosts.filter(|candidate| candidate.name() == scope.host);
    let hosted = own.flat_map(unit_stacks);
    let reserved = outcome.state().reserved().iter();
    let opted = reserved.filter(|stack| scope.wants_reserved(stack));
    let parent = RepoPath::reserved_root();
    let kept = opted.map(move |stack| Unit {
        directory: parent.listed(stack.name().as_str()),
        stack,
    });
    hosted
        .chain(kept)
        .filter(|unit| unit.stack.policy().state() == StackState::Active)
        .collect()
}

fn unit_stacks(host: &Host) -> impl Iterator<Item = Unit<'_>> {
    let parent = RepoPath::hosts_root().listed(host.name().as_str());
    host.stacks().iter().map(move |stack| Unit {
        directory: parent.listed(stack.name().as_str()),
        stack,
    })
}

async fn deploy_all<Docker: ComposeRunner + Sync>(
    compose: &Docker,
    units: &[Unit<'_>],
    commit: &CommitSha,
) -> Vec<StackReport> {
    let mut reports = Vec::new();
    for unit in units {
        reports.push(deploy(compose, unit, commit).await);
    }
    reports
}

async fn deploy<Docker: ComposeRunner + Sync>(
    compose: &Docker,
    unit: &Unit<'_>,
    commit: &CommitSha,
) -> StackReport {
    let deployment = match request_for(unit, commit) {
        Ok(request) => run_stack(compose, &request).await,
        Err(error) => Deployment::Failed(Detail::new(&error.to_string())),
    };
    StackReport::new(unit.directory.clone(), deployment)
}

async fn run_stack<Docker: ComposeRunner + Sync>(
    compose: &Docker,
    request: &DeployRequest,
) -> Deployment {
    match compose.converge(request).await {
        Ok(()) => Deployment::Running,
        Err(error) => Deployment::Failed(Detail::new(&error.to_string())),
    }
}

fn request_for(unit: &Unit<'_>, commit: &CommitSha) -> Result<DeployRequest, ValueError> {
    let project = ProjectName::of(unit.stack.name())?;
    let target = Target::new(project, unit.directory.clone());
    let sources = Extras::default().with_sources(unit.stack.compose().clone());
    let request = DeployRequest::new(target, commit.clone(), built_by(unit.stack));
    Ok(request.with_extras(sources))
}

fn built_by(stack: &Stack) -> BuiltServices {
    let services = stack.compose().services().iter();
    let built = services.filter(|service| service.source().build().is_some());
    built.map(|service| service.name().clone()).collect()
}

/// The rejected stacks of `host`, reported as invalid.
fn invalid(outcome: &Outcome, scope: &Scope<'_>) -> StackReports {
    let own = outcome
        .rejected()
        .iter()
        .filter(|rejection| belongs(rejection, scope));
    own.map(|rejection| {
        let violations = rejection.violations().clone();
        StackReport::new(
            rejection.directory().clone(),
            Deployment::Invalid(violations),
        )
    })
    .collect()
}

/// Whether `rejection` is about the host directory, one of its stacks or an opted-in reserved
/// stack.
fn belongs(rejection: &Rejection, scope: &Scope<'_>) -> bool {
    let directory = rejection.directory();
    let own = RepoPath::hosts_root().listed(scope.host.as_str());
    let reserved = RepoPath::reserved_root();
    let opted = scope
        .reserved
        .iter()
        .any(|name| *directory == reserved.listed(name.as_str()));
    directory.is_within(&own) || opted
}
