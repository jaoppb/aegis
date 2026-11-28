//! Removal over the real repository adapters, with fake Docker, node and purge ports: what leaves
//! git is taken down and keeps its data, and only a tombstone deletes anything.

use std::future::{Future, ready};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use aegis_adapters::{FsRepo, SerdeStackParser, SopsCheck};
use aegis_application::{
    Checkout, ComposeError, ComposeRunner, DeployRequest, GitError, NodeDeleter, OrphanSweep,
    Projects, Purge, Purger, Reason, Reconcile, ReconcilePorts, Reconciler, Remover, Repository,
    SafePath, Teardown,
};
use aegis_domain::{
    CommitSha, Deployment, Detail, HostName, Leftover, Leftovers, NodeLabel, ProjectName, Remains,
    Report, Tombstone,
};

/// A checkout in a scratch directory, written file by file.
#[cfg(test)]
struct Scratch {
    root: PathBuf,
}

#[cfg(test)]
async fn write_files(root: &std::path::Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let file = root.join(path);
        tokio::fs::create_dir_all(file.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(file, text).await.unwrap();
    }
}

#[cfg(test)]
impl Scratch {
    async fn new(name: &str, files: &[(&str, &str)]) -> Self {
        let root =
            std::env::temp_dir().join(format!("aegis-removal-{}-{name}", std::process::id()));
        tokio::fs::remove_dir_all(&root).await.ok();
        write_files(&root, files).await;
        Self { root }
    }
}

#[cfg(test)]
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
}

/// Every port of removal, recording each call in order. `failing` names the step that fails.
#[cfg(test)]
#[derive(Clone, Default)]
struct Fake {
    events: Arc<Mutex<Vec<String>>>,
    leftovers: Vec<(&'static str, &'static [&'static str])>,
    failing: Option<&'static str>,
}

#[cfg(test)]
impl Fake {
    fn log(&self, step: &str, text: &str) {
        self.events.lock().unwrap().push(format!("{step} {text}"));
    }

    fn record(&self, step: &'static str, text: &str) -> Result<(), Reason> {
        self.log(step, text);
        let broken = self.failing == Some(step);
        (!broken)
            .then_some(())
            .ok_or_else(|| Reason::new(&"the fake failed"))
    }

    fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl Projects for Fake {
    fn leftovers(&self) -> impl Future<Output = Result<Leftovers, Reason>> + Send {
        let found = self
            .leftovers
            .iter()
            .map(|each| leftover_of(each.0, each.1));
        ready(Ok(Leftovers::new(found.collect())))
    }
}

#[cfg(test)]
fn destroys(event: &str) -> bool {
    ["node", "volumes", "path"]
        .iter()
        .any(|step| event.starts_with(step))
}

#[cfg(test)]
fn leftover_of(project: &str, volumes: &[&str]) -> Leftover {
    let lines = volumes
        .iter()
        .map(|name| Detail::new(&format!("volume {name}")));
    let name = ProjectName::parse(project).unwrap();
    Leftover::new(name, Remains::new(lines.collect()))
}

#[cfg(test)]
impl Teardown for Fake {
    fn down(&self, project: &ProjectName) -> impl Future<Output = Result<(), Reason>> + Send {
        ready(self.record("down", &project.to_string()))
    }
}

#[cfg(test)]
impl NodeDeleter for Fake {
    fn delete_node(&self, node: &NodeLabel) -> impl Future<Output = Result<(), Reason>> + Send {
        ready(self.record("node", &node.to_string()))
    }
}

#[cfg(test)]
impl Purger for Fake {
    fn remove_volumes(
        &self,
        tombstone: &Tombstone,
    ) -> impl Future<Output = Result<(), Reason>> + Send {
        ready(self.record("volumes", &tombstone.project().to_string()))
    }

    fn remove_path(
        &self,
        _tombstone: &Tombstone,
        path: &SafePath,
    ) -> impl Future<Output = Result<(), Reason>> + Send {
        ready(self.record("path", &path.path().to_string()))
    }
}

#[cfg(test)]
#[derive(Clone, Default)]
struct FakeCompose {
    deployed: Arc<Mutex<Vec<String>>>,
}

#[cfg(test)]
impl ComposeRunner for FakeCompose {
    fn converge(
        &self,
        request: &DeployRequest,
    ) -> impl Future<Output = Result<(), ComposeError>> + Send {
        let project = request.target().project().to_string();
        self.deployed.lock().unwrap().push(project);
        ready(Ok(()))
    }
}

#[cfg(test)]
struct FakeGit;

#[cfg(test)]
impl Checkout for FakeGit {
    fn sync(&self, _commit: &CommitSha) -> impl Future<Output = Result<(), GitError>> + Send {
        ready(Ok(()))
    }
}

#[cfg(test)]
type Removing = Remover<OrphanSweep<Fake, Fake>, Purge<Fake, Fake, Fake>>;

#[cfg(test)]
async fn reconcile(files: &Scratch, fake: &Fake, compose: &FakeCompose) -> Report {
    let repository = Repository::new(FsRepo::new(&files.root), SerdeStackParser, SopsCheck);
    let ports = ReconcilePorts::new(FakeGit, compose.clone(), repository);
    let host = HostName::new("host").unwrap();
    let sweep = OrphanSweep::new(host.clone(), fake.clone(), fake.clone());
    let removal: Removing =
        Remover::new(sweep, Purge::new(fake.clone(), fake.clone(), fake.clone()));
    let reconciler = Reconciler::new(host, ports).with_removal(removal);
    let commit = CommitSha::new("abc1234").unwrap();
    reconciler.reconcile(commit).await.unwrap()
}

#[cfg(test)]
fn deployment<'report>(report: &'report Report, directory: &str) -> &'report Deployment {
    let found = report
        .stacks()
        .iter()
        .find(|stack| stack.directory().as_str() == directory);
    found.unwrap().deployment()
}

#[cfg(test)]
const WEB: (&str, &str) = (
    "hosts/host/web/compose.yaml",
    "services:\n  web:\n    image: nginx:1\n",
);

#[cfg(test)]
async fn checkout(name: &str, files: &[(&str, &str)]) -> Scratch {
    Scratch::new(name, files).await
}

#[cfg(test)]
async fn tombstoned(name: &str, compose: &str, policy: &str) -> Scratch {
    checkout(
        name,
        &[
            WEB,
            ("hosts/host/old/aegis.toml", policy),
            ("hosts/host/old/compose.yaml", compose),
        ],
    )
    .await
}

#[cfg(test)]
const OLD_COMPOSE: &str = "services:\n  app:\n    image: nginx:1\n    volumes:\n      - data:/var/lib/app\n      - /srv/data/old:/mnt/old\n      - /srv/keep/old:/mnt/keep:ro\nvolumes:\n  data: {}\n";

#[cfg(test)]
const PURGED: &str = "state = \"purged\"\n";

#[cfg(test)]
const PURGED_EXPOSED: &str =
    "state = \"purged\"\ntailnet_tags = [\"tag:app\"]\n\n[expose]\nservice = \"app\"\nport = 80\n";

#[test]
fn a_stack_deleted_from_git_is_taken_down_and_its_data_is_kept() {
    runtime().block_on(async {
        let files = checkout("deleted", &[WEB]).await;
        let fake = Fake {
            leftovers: vec![("aegis-old", &["aegis-old_data"])],
            ..Fake::default()
        };
        let report = reconcile(&files, &fake, &FakeCompose::default()).await;
        assert_eq!(fake.events(), ["down aegis-old"]);
        let kept = Remains::new(vec![Detail::new("volume aegis-old_data")]);
        assert_eq!(
            deployment(&report, "hosts/host/old"),
            &Deployment::Orphaned(kept)
        );
    });
}

#[test]
fn a_deleted_stack_never_reaches_the_purger_even_when_it_has_data() {
    runtime().block_on(async {
        let files = checkout("never", &[WEB]).await;
        let fake = Fake {
            leftovers: vec![("aegis-old", &["aegis-old_data", "aegis-old_more"])],
            ..Fake::default()
        };
        reconcile(&files, &fake, &FakeCompose::default()).await;
        assert!(!fake.events().iter().any(|event| destroys(event)));
    });
}

#[test]
fn a_stack_still_in_git_is_not_an_orphan() {
    runtime().block_on(async {
        let files = checkout("declared", &[WEB]).await;
        let fake = Fake {
            leftovers: vec![("aegis-web", &["aegis-web_data"])],
            ..Fake::default()
        };
        reconcile(&files, &fake, &FakeCompose::default()).await;
        assert!(fake.events().is_empty());
    });
}

#[test]
fn a_rejected_stack_is_not_an_orphan() {
    runtime().block_on(async {
        let broken = (
            "hosts/host/web/compose.yaml",
            "services:\n  web:\n    volumes:\n      - ./missing:/x\n",
        );
        let files = checkout("rejected", &[broken]).await;
        let fake = Fake {
            leftovers: vec![("aegis-web", &[])],
            ..Fake::default()
        };
        reconcile(&files, &fake, &FakeCompose::default()).await;
        assert!(fake.events().is_empty());
    });
}

#[test]
fn a_reserved_stack_is_never_an_orphan_even_when_the_host_did_not_opt_in() {
    runtime().block_on(async {
        let agent = (
            "aegis/agent/compose.yaml",
            "services:\n  agent:\n    image: x:1\n",
        );
        let files = checkout("reserved", &[WEB, agent]).await;
        let fake = Fake {
            leftovers: vec![("aegis-agent", &[])],
            ..Fake::default()
        };
        reconcile(&files, &fake, &FakeCompose::default()).await;
        assert!(fake.events().is_empty());
    });
}

#[test]
fn nothing_is_taken_down_when_the_hosts_directory_cannot_be_read() {
    runtime().block_on(async {
        let files = checkout("unreadable", &[("hosts", "not a directory")]).await;
        let fake = Fake {
            leftovers: vec![("aegis-web", &[])],
            ..Fake::default()
        };
        reconcile(&files, &fake, &FakeCompose::default()).await;
        assert!(fake.events().is_empty());
    });
}

#[test]
fn a_tombstone_takes_the_stack_down_then_removes_node_volumes_and_paths_in_order() {
    runtime().block_on(async {
        let files = tombstoned("purge", OLD_COMPOSE, PURGED_EXPOSED).await;
        let fake = Fake::default();
        let compose = FakeCompose::default();
        let report = reconcile(&files, &fake, &compose).await;
        let expected = [
            "down aegis-old",
            "node app",
            "volumes aegis-old",
            "path /srv/data/old",
        ];
        assert_eq!(fake.events(), expected);
        assert_eq!(deployment(&report, "hosts/host/old"), &Deployment::Purged);
        assert_eq!(*compose.deployed.lock().unwrap(), ["aegis-web"]);
    });
}

#[test]
fn a_tombstone_of_a_stack_that_exposes_nothing_removes_no_node() {
    runtime().block_on(async {
        let files = tombstoned("nonode", OLD_COMPOSE, PURGED).await;
        let fake = Fake::default();
        reconcile(&files, &fake, &FakeCompose::default()).await;
        assert!(!fake.events().iter().any(|event| event.starts_with("node")));
    });
}

#[test]
fn a_read_only_host_path_is_never_deleted() {
    runtime().block_on(async {
        let files = tombstoned("readonly", OLD_COMPOSE, PURGED).await;
        let fake = Fake::default();
        reconcile(&files, &fake, &FakeCompose::default()).await;
        assert!(
            !fake
                .events()
                .iter()
                .any(|event| event.contains("/srv/keep"))
        );
    });
}

#[test]
fn a_failing_step_stops_the_purge_before_any_data_is_deleted() {
    runtime().block_on(async {
        let files = tombstoned("failing", OLD_COMPOSE, PURGED_EXPOSED).await;
        let fake = Fake {
            failing: Some("node"),
            ..Fake::default()
        };
        let report = reconcile(&files, &fake, &FakeCompose::default()).await;
        assert_eq!(fake.events(), ["down aegis-old", "node app"]);
        assert!(matches!(
            deployment(&report, "hosts/host/old"),
            Deployment::Failed(_)
        ));
    });
}

#[test]
fn a_path_too_close_to_the_root_refuses_the_whole_purge_before_anything_runs() {
    runtime().block_on(async {
        let compose =
            "services:\n  app:\n    image: nginx:1\n    volumes:\n      - /home/jao:/mnt\n";
        let files = tombstoned("shallow", compose, PURGED_EXPOSED).await;
        let fake = Fake::default();
        let report = reconcile(&files, &fake, &FakeCompose::default()).await;
        assert!(fake.events().is_empty());
        assert!(matches!(
            deployment(&report, "hosts/host/old"),
            Deployment::Failed(_)
        ));
    });
}

#[test]
fn a_path_another_stack_mounts_is_never_deleted() {
    runtime().block_on(async {
        let other = (
            "hosts/host/web/compose.yaml",
            "services:\n  web:\n    image: nginx:1\n    volumes:\n      - /srv/data:/shared:ro\n",
        );
        let files = checkout(
            "shared",
            &[
                other,
                ("hosts/host/old/aegis.toml", PURGED),
                ("hosts/host/old/compose.yaml", OLD_COMPOSE),
            ],
        )
        .await;
        let fake = Fake::default();
        let report = reconcile(&files, &fake, &FakeCompose::default()).await;
        assert!(fake.events().is_empty());
        assert!(matches!(
            deployment(&report, "hosts/host/old"),
            Deployment::Failed(_)
        ));
    });
}

#[test]
fn host_paths_are_not_deleted_while_a_stack_of_the_host_is_rejected() {
    runtime().block_on(async {
        let broken = (
            "hosts/host/web/compose.yaml",
            "services:\n  web:\n    volumes:\n      - ./missing:/x\n",
        );
        let files = checkout(
            "unverified",
            &[
                broken,
                ("hosts/host/old/aegis.toml", PURGED),
                ("hosts/host/old/compose.yaml", OLD_COMPOSE),
            ],
        )
        .await;
        let fake = Fake::default();
        reconcile(&files, &fake, &FakeCompose::default()).await;
        assert!(fake.events().is_empty());
    });
}

#[test]
fn an_active_stack_never_reaches_the_purger() {
    runtime().block_on(async {
        let files = checkout("active", &[WEB]).await;
        let fake = Fake::default();
        reconcile(&files, &fake, &FakeCompose::default()).await;
        assert!(fake.events().is_empty());
    });
}

#[test]
fn a_tombstone_is_repeated_on_every_reconcile_until_the_directory_is_deleted() {
    runtime().block_on(async {
        let files = tombstoned("again", OLD_COMPOSE, PURGED).await;
        let fake = Fake::default();
        reconcile(&files, &fake, &FakeCompose::default()).await;
        reconcile(&files, &fake, &FakeCompose::default()).await;
        assert_eq!(fake.events().len(), 6);
    });
}
