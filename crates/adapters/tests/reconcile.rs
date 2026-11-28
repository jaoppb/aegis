//! The reconcile use case over the real repository adapters, with fake git and compose ports.

use std::future::{Future, ready};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use aegis_adapters::{FsRepo, SerdeStackParser, SopsCheck};
use aegis_application::{
    Checkout, ComposeError, ComposeRunner, DeployRequest, GitError, Ports, Reason, Reconcile,
    ReconcileError, Reconciler, Repository,
};
use aegis_domain::{CommitSha, Deployment, HostName, Report, ServiceName, StackReport};

#[cfg(test)]
#[derive(Clone, Default)]
struct FakeCompose {
    deployed: Arc<Mutex<Vec<DeployRequest>>>,
    failing: Vec<&'static str>,
}

#[cfg(test)]
impl FakeCompose {
    fn failing(projects: &[&'static str]) -> Self {
        Self {
            failing: projects.to_vec(),
            ..Self::default()
        }
    }

    fn requests(&self) -> Vec<DeployRequest> {
        self.deployed.lock().unwrap().clone()
    }

    fn projects(&self) -> Vec<String> {
        let requests = self.requests();
        requests
            .iter()
            .map(|request| request.target().project().to_string())
            .collect()
    }
}

#[cfg(test)]
impl ComposeRunner for FakeCompose {
    fn converge(
        &self,
        request: &DeployRequest,
    ) -> impl Future<Output = Result<(), ComposeError>> + Send {
        self.deployed.lock().unwrap().push(request.clone());
        let project = request.target().project().to_string();
        let broken = self.failing.iter().any(|name| *name == project);
        let reason = Reason::new(&format!("{project} is broken"));
        ready(
            (!broken)
                .then_some(())
                .ok_or_else(|| ComposeError::new(reason)),
        )
    }
}

#[cfg(test)]
#[derive(Clone, Default)]
struct FakeGit {
    synced: Arc<Mutex<Vec<String>>>,
    failure: Option<&'static str>,
}

#[cfg(test)]
impl Checkout for FakeGit {
    fn sync(&self, commit: &CommitSha) -> impl Future<Output = Result<(), GitError>> + Send {
        self.synced.lock().unwrap().push(commit.to_string());
        let failed = |text: &str| Err(GitError::new(Reason::new(&text)));
        ready(self.failure.map_or(Ok(()), failed))
    }
}

#[cfg(test)]
fn reconciler(git: &FakeGit, compose: &FakeCompose) -> Reconciler<FakeGit, FakeCompose> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/reconcile");
    let repository = Repository::new(FsRepo::new(root), SerdeStackParser, SopsCheck);
    let ports = Ports::new(git.clone(), compose.clone(), repository);
    Reconciler::new(HostName::new("host").unwrap(), ports)
}

#[cfg(test)]
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
}

#[cfg(test)]
fn commit() -> CommitSha {
    CommitSha::new("abc1234").unwrap()
}

#[cfg(test)]
fn reconcile(git: &FakeGit, compose: &FakeCompose) -> Result<Report, ReconcileError> {
    runtime().block_on(reconciler(git, compose).reconcile(commit()))
}

#[cfg(test)]
fn stack<'report>(report: &'report Report, directory: &str) -> &'report Deployment {
    let found = report
        .stacks()
        .iter()
        .find(|stack| stack.directory().as_str() == directory);
    found.map(StackReport::deployment).unwrap()
}

#[test]
fn the_checkout_moves_to_the_commit_before_anything_is_read() {
    let git = FakeGit::default();
    reconcile(&git, &FakeCompose::default()).unwrap();
    assert_eq!(*git.synced.lock().unwrap(), ["abc1234"]);
}

#[test]
fn only_the_valid_active_stacks_of_the_host_are_deployed() {
    let compose = FakeCompose::default();
    reconcile(&FakeGit::default(), &compose).unwrap();
    assert_eq!(compose.projects(), ["aegis-builder", "aegis-web"]);
}

#[test]
fn applying_the_same_state_twice_deploys_the_same_requests() {
    let compose = FakeCompose::default();
    let first = reconcile(&FakeGit::default(), &compose).unwrap();
    let second = reconcile(&FakeGit::default(), &compose).unwrap();
    let requests = compose.requests();
    let (earlier, later) = requests.split_at(requests.len() / 2);
    assert_eq!(first, second);
    assert_eq!(earlier, later);
}

#[test]
fn one_failing_stack_does_not_block_the_others() {
    let compose = FakeCompose::failing(&["aegis-builder"]);
    let report = reconcile(&FakeGit::default(), &compose).unwrap();
    let failed = stack(&report, "hosts/host/builder");
    assert!(matches!(failed, Deployment::Failed(detail) if detail.as_str().contains("is broken")));
    assert_eq!(stack(&report, "hosts/host/web"), &Deployment::Running);
}

#[test]
fn an_invalid_stack_is_reported_and_never_deployed() {
    let compose = FakeCompose::default();
    let report = reconcile(&FakeGit::default(), &compose).unwrap();
    let invalid = stack(&report, "hosts/host/broken");
    assert!(matches!(invalid, Deployment::Invalid(violations) if !violations.is_empty()));
    assert!(!compose.projects().contains(&"aegis-broken".to_owned()));
}

#[test]
fn a_stack_compose_cannot_name_a_project_for_fails_without_stopping_the_rest() {
    let report = reconcile(&FakeGit::default(), &FakeCompose::default()).unwrap();
    assert!(matches!(
        stack(&report, "hosts/host/Upper"),
        Deployment::Failed(_)
    ));
    assert_eq!(stack(&report, "hosts/host/web"), &Deployment::Running);
}

#[test]
fn built_services_are_handed_to_compose_with_the_commit() {
    let compose = FakeCompose::default();
    reconcile(&FakeGit::default(), &compose).unwrap();
    let requests = compose.requests();
    let builder = requests
        .iter()
        .find(|request| request.target().project().as_str() == "aegis-builder");
    let request = builder.unwrap();
    let built: Vec<&str> = request
        .built()
        .as_slice()
        .iter()
        .map(ServiceName::as_str)
        .collect();
    assert_eq!(built, ["App"]);
    assert_eq!(request.commit().as_str(), "abc1234");
    assert_eq!(request.target().directory().as_str(), "hosts/host/builder");
}

#[test]
fn another_hosts_stacks_and_purged_stacks_are_left_alone() {
    let report = reconcile(&FakeGit::default(), &FakeCompose::default()).unwrap();
    let listed: Vec<&str> = report
        .stacks()
        .iter()
        .map(|stack| stack.directory().as_str())
        .collect();
    assert!(
        !listed
            .iter()
            .any(|directory| directory.contains("elsewhere"))
    );
    assert!(!listed.contains(&"hosts/host/gone"));
}

#[test]
fn a_checkout_failure_deploys_nothing() {
    let git = FakeGit {
        failure: Some("no such commit"),
        ..FakeGit::default()
    };
    let compose = FakeCompose::default();
    let result = reconcile(&git, &compose);
    assert!(matches!(result, Err(ReconcileError::Checkout(_))));
    assert!(compose.requests().is_empty());
}
