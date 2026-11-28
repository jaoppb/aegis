//! The Docker adapters of removal against a fake `docker` that records every call: a teardown never
//! deletes data, discovery only reads, and a purge deletes only what is labelled as the project's.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use aegis_adapters::{DockerCompose, DockerProjects, DockerPurger};
use aegis_application::{Projects, Purger, SafePath, Teardown};
use aegis_domain::{
    ComposeSource, ComposeSummary, HostPath, HostPaths, ProjectName, RepoPath, Stack,
    StackDefinition, StackIdentity, StackKind, StackName, StackPolicy, StackState, TailnetTags,
    Tombstone,
};

/// A scratch directory holding a fake `docker`. It answers `ps` and `volume ls` from fixed text.
#[cfg(test)]
struct Sandbox {
    root: PathBuf,
}

#[cfg(test)]
impl Sandbox {
    async fn new(name: &str, containers: &str, volumes: &str) -> Self {
        let root = std::env::temp_dir().join(format!("aegis-dr-{}-{name}", std::process::id()));
        tokio::fs::remove_dir_all(&root).await.ok();
        tokio::fs::create_dir_all(&root).await.unwrap();
        let script = format!(
            "#!/bin/sh\necho \"ARGS:$*\" >> {log}\ncase \"$1 $2\" in\n\
             \"ps -a\") printf '%s' '{containers}' ;;\n\
             \"volume ls\") printf '%s' '{volumes}' ;;\nesac\nexit 0\n",
            log = root.join("log").display()
        );
        tokio::fs::write(root.join("docker"), script).await.unwrap();
        let executable = std::fs::Permissions::from_mode(0o755);
        tokio::fs::set_permissions(root.join("docker"), executable)
            .await
            .unwrap();
        Self { root }
    }

    fn program(&self) -> String {
        self.root.join("docker").display().to_string()
    }

    async fn calls(&self) -> Vec<String> {
        let text = tokio::fs::read_to_string(self.root.join("log"))
            .await
            .unwrap_or_default();
        text.lines().map(str::to_owned).collect()
    }
}

#[cfg(test)]
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[cfg(test)]
fn tombstone() -> Tombstone {
    let policy = StackPolicy::new(None, TailnetTags::default(), StackState::Purged);
    let identity = StackIdentity::new(StackName::new("old").unwrap(), StackKind::Host);
    let source = ComposeSource::new("services: {}\n");
    let definition = StackDefinition::new(policy, source, ComposeSummary::default());
    let stack = Stack::new(identity, definition);
    Tombstone::of(RepoPath::new("hosts/host/old").unwrap(), &stack).unwrap()
}

#[test]
fn taking_a_project_down_never_removes_volumes() {
    runtime().block_on(async {
        let sandbox = Sandbox::new("down", "", "").await;
        let compose = DockerCompose::with_program(sandbox.program(), &sandbox.root);
        let project = ProjectName::parse("aegis-old").unwrap();
        compose.down(&project).await.unwrap();
        let calls = sandbox.calls().await;
        assert_eq!(calls, ["ARGS:compose -p aegis-old down"]);
    });
}

#[test]
fn discovery_reads_only_and_lists_projects_under_the_host_directory_with_their_volumes() {
    runtime().block_on(async {
        let containers = "c1\taegis-old\t/checkout/hosts/host/old\nc2\taegis-web\t/checkout/hosts/host/web\nc3\taegis-far\t/checkout/hosts/other/far\nc4\tother\t/somewhere\n";
        let volumes = "aegis-old_data\taegis-old\naegis-gone_data\taegis-gone\n";
        let sandbox = Sandbox::new("discover", containers, volumes).await;
        let projects = DockerProjects::with_program(sandbox.program(), "/checkout/hosts/host");
        let found = projects.leftovers().await.unwrap();
        let names: Vec<String> = found.iter().map(|each| each.project().to_string()).collect();
        assert_eq!(names, ["aegis-gone", "aegis-old", "aegis-web"]);
        let calls = sandbox.calls().await;
        assert!(calls.iter().all(|call| !call.contains(" rm") && !call.contains("run ")));
    });
}

#[test]
fn a_project_with_a_container_outside_the_host_directory_is_not_a_leftover() {
    runtime().block_on(async {
        let containers =
            "c1\taegis-mixed\t/checkout/hosts/host/mixed\nc2\taegis-mixed\t/elsewhere\n";
        let sandbox = Sandbox::new("mixed", containers, "aegis-mixed_data\taegis-mixed\n").await;
        let projects = DockerProjects::with_program(sandbox.program(), "/checkout/hosts/host");
        assert!(projects.leftovers().await.unwrap().is_empty());
    });
}

#[test]
fn the_agents_own_container_and_its_project_are_never_leftovers() {
    runtime().block_on(async {
        let containers = "abcdef123456789\taegis-agent\t/checkout/hosts/host/agent\n";
        let sandbox = Sandbox::new("own", containers, "").await;
        let projects = DockerProjects::with_program(sandbox.program(), "/checkout/hosts/host")
            .excluding("abcdef123456");
        assert!(projects.leftovers().await.unwrap().is_empty());
    });
}

#[test]
fn an_empty_own_container_id_excludes_nothing() {
    runtime().block_on(async {
        let containers = "abc\taegis-old\t/checkout/hosts/host/old\n";
        let sandbox = Sandbox::new("noown", containers, "").await;
        let projects =
            DockerProjects::with_program(sandbox.program(), "/checkout/hosts/host").excluding("");
        assert_eq!(projects.leftovers().await.unwrap().len(), 1);
    });
}

#[test]
fn a_purge_removes_only_the_volumes_labelled_with_the_project() {
    runtime().block_on(async {
        let sandbox = Sandbox::new("volumes", "", "aegis-old_data\naegis-old_state\n").await;
        let purger = DockerPurger::with_program(sandbox.program());
        purger.remove_volumes(&tombstone()).await.unwrap();
        let calls = sandbox.calls().await;
        assert_eq!(calls.len(), 2);
        assert!(calls[0].contains("label=com.docker.compose.project=aegis-old"));
        assert_eq!(calls[1], "ARGS:volume rm aegis-old_data aegis-old_state");
    });
}

#[test]
fn a_purge_with_no_labelled_volume_removes_nothing() {
    runtime().block_on(async {
        let sandbox = Sandbox::new("novolumes", "", "").await;
        DockerPurger::with_program(sandbox.program())
            .remove_volumes(&tombstone())
            .await
            .unwrap();
        assert_eq!(sandbox.calls().await.len(), 1);
    });
}

#[test]
fn a_path_is_deleted_in_a_container_that_walks_it_segment_by_segment() {
    runtime().block_on(async {
        let sandbox = Sandbox::new("path", "", "").await;
        let path = HostPath::new("/srv/data/web").unwrap();
        let safe = SafePath::check(&path, &HostPaths::default()).unwrap();
        DockerPurger::with_program(sandbox.program())
            .remove_path(&tombstone(), &safe)
            .await
            .unwrap();
        let calls = sandbox.calls().await;
        assert_eq!(calls.len(), 1);
        assert!(calls[0].contains("-v /:/host"), "{}", calls[0]);
        assert!(calls[0].contains("--network none"));
        assert!(calls[0].ends_with("sh /srv/data/web /host"), "{}", calls[0]);
    });
}

/// Runs the delete script for `path` against a host root at `root`, and reports its exit code.
#[cfg(test)]
async fn run_script(path: &str, root: &std::path::Path) -> Option<i32> {
    let status = tokio::process::Command::new("sh")
        .args(["-c", aegis_adapters::REMOVE_SCRIPT, "sh", path])
        .arg(root)
        .status()
        .await
        .unwrap();
    status.code()
}

#[cfg(test)]
async fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("aegis-script-{}-{name}", std::process::id()));
    tokio::fs::remove_dir_all(&root).await.ok();
    tokio::fs::create_dir_all(root.join("srv/data/web/inner"))
        .await
        .unwrap();
    root
}

#[test]
fn the_delete_script_removes_a_plain_directory() {
    runtime().block_on(async {
        let root = scratch("plain").await;
        assert_eq!(run_script("/srv/data/web", &root).await, Some(0));
        assert!(!root.join("srv/data/web").exists());
        assert!(root.join("srv/data").exists());
    });
}

#[test]
fn the_delete_script_treats_a_missing_path_as_deleted() {
    runtime().block_on(async {
        let root = scratch("missing").await;
        assert_eq!(run_script("/srv/data/gone", &root).await, Some(0));
    });
}

#[test]
fn the_delete_script_never_follows_a_symlink_in_any_segment() {
    runtime().block_on(async {
        let root = scratch("link").await;
        tokio::fs::create_dir_all(root.join("elsewhere/web"))
            .await
            .unwrap();
        std::os::unix::fs::symlink(root.join("elsewhere"), root.join("srv/link")).unwrap();
        assert_eq!(run_script("/srv/link/web", &root).await, Some(2));
        assert!(root.join("elsewhere/web").exists());
        assert_eq!(run_script("/srv/link", &root).await, Some(2));
        assert!(root.join("elsewhere/web").exists());
    });
}

#[test]
fn the_delete_script_leaves_a_file_alone() {
    runtime().block_on(async {
        let root = scratch("file").await;
        tokio::fs::write(root.join("srv/data/file"), "x")
            .await
            .unwrap();
        assert_eq!(run_script("/srv/data/file", &root).await, Some(3));
        assert!(root.join("srv/data/file").exists());
    });
}
