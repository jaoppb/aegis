//! The state probe against a fake `docker` executable that exits as told.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use aegis_adapters::DockerStateProbe;
use aegis_application::{StateProbe, StateVolume};
use aegis_domain::{ProjectName, StackName};

#[cfg(test)]
async fn fake(name: &str, exit_code: i32) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("aegis-{}-{name}", std::process::id()));
    tokio::fs::remove_dir_all(&root).await.ok();
    tokio::fs::create_dir_all(&root).await.unwrap();
    let log = root.join("log");
    let script = format!(
        "#!/bin/sh\necho \"$*\" >> {}\nexit {exit_code}\n",
        log.display()
    );
    let program = root.join("docker");
    tokio::fs::write(&program, script).await.unwrap();
    let executable = std::fs::Permissions::from_mode(0o755);
    tokio::fs::set_permissions(&program, executable)
        .await
        .unwrap();
    (program, log)
}

#[cfg(test)]
fn project() -> ProjectName {
    ProjectName::of(&StackName::new("vaultwarden").unwrap()).unwrap()
}

#[test]
fn a_state_file_in_the_volume_means_the_node_has_joined() {
    runtime().block_on(async {
        let (program, log) = fake("populated", 0).await;
        let probe = DockerStateProbe::with_program(program.display().to_string());
        assert_eq!(probe.probe(&project()).await, Ok(StateVolume::Populated));
        let ran = tokio::fs::read_to_string(log).await.unwrap();
        assert!(
            ran.contains("-v aegis-vaultwarden_tailscale-state:/var/lib/tailscale:ro"),
            "{ran}"
        );
        assert!(
            ran.contains("--network none")
                && ran.contains("-s /var/lib/tailscale/tailscaled.state")
        );
    });
}

#[test]
fn no_state_file_means_the_node_has_never_joined() {
    runtime().block_on(async {
        let (program, _) = fake("empty", 1).await;
        let probe = DockerStateProbe::with_program(program.display().to_string());
        assert_eq!(probe.probe(&project()).await, Ok(StateVolume::Empty));
    });
}

#[test]
fn a_docker_failure_is_an_error_and_never_an_empty_volume() {
    runtime().block_on(async {
        let (program, _) = fake("broken", 125).await;
        let probe = DockerStateProbe::with_program(program.display().to_string());
        let error = probe.probe(&project()).await.unwrap_err();
        assert!(error.to_string().contains("125"), "{error}");
    });
}

#[test]
fn a_missing_docker_is_an_error() {
    runtime().block_on(async {
        let probe = DockerStateProbe::with_program("/nonexistent/aegis-docker");
        assert!(probe.probe(&project()).await.is_err());
    });
}

#[cfg(test)]
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}
