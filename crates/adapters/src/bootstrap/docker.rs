//! The `docker` CLI adapters of bootstrap: the check that a sidecar's state volume holds a node,
//! and the compose runner for the stacks rendered below the bootstrap directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use aegis_application::{
    AuthKey, BootstrapError, BuiltServices, ComposeRunner, DeployRequest, Environment, ExtraFiles,
    Extras, Reason, SidecarState, StackParser, StackRunner, StateVolume, Target, sidecar_volume,
};
use aegis_domain::{CommitSha, HostName, ProjectName, RepoPath, StackName};
use tokio::process::Command;

use crate::cli;
use crate::{
    DecryptingCompose, DockerCompose, SecretPorts, SerdeStackParser, SopsDecrypt, TmpfsStore,
};
use aegis_application::render::TAILSCALE_IMAGE;

/// How long a `docker` query may take; the first one may pull the Tailscale image.
const DOCKER_TIMEOUT: Duration = Duration::from_mins(5);

/// The file the Tailscale daemon keeps its node identity in.
const STATE_FILE: &str = "/state/tailscaled.state";

/// The variable the sidecar reads its single-use key from.
const KEY_VARIABLE: &str = "TS_AUTHKEY";

/// The commit a bootstrap deploy names. No service is built, so no image is tagged with it.
const NO_BUILD_COMMIT: &str = "0000000";

/// Reads the state volume of a sidecar through the local Docker daemon.
#[derive(Debug, Clone)]
pub struct DockerVolumes {
    program: String,
}

impl DockerVolumes {
    /// Query `docker` from the `PATH`.
    #[must_use]
    pub fn new() -> Self {
        Self::with_program("docker")
    }

    /// Query with the `program` executable.
    #[must_use]
    pub fn with_program(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
        }
    }
}

impl Default for DockerVolumes {
    fn default() -> Self {
        Self::new()
    }
}

fn docker(program: &str, arguments: &[&str]) -> Command {
    let mut command = Command::new(program);
    command.args(arguments);
    cli::inherit_only(&mut command, &cli::COMPOSE_INHERITED);
    command
}

async fn query(program: &str, arguments: &[&str]) -> Result<Output, Reason> {
    cli::run(docker(program, arguments), String::new(), DOCKER_TIMEOUT).await
}

fn failed(output: &Output) -> Reason {
    Reason::new(&format!("docker exited with {}", output.status))
}

async fn volume_exists(program: &str, name: &str) -> Result<bool, Reason> {
    let filter = format!("name={name}");
    let output = query(program, &["volume", "ls", "--quiet", "--filter", &filter]).await?;
    cli::succeeded(&output, "docker")?;
    let listed = String::from_utf8_lossy(&output.stdout);
    Ok(listed.lines().any(|line| line.trim() == name))
}

async fn volume_state(program: &str, name: &str) -> Result<StateVolume, Reason> {
    let mount = format!("{name}:/state:ro");
    let arguments = [
        "run",
        "--rm",
        "--entrypoint",
        "test",
        "-v",
        &mount,
        TAILSCALE_IMAGE,
    ];
    let output = query(program, &[&arguments[..], &["-s", STATE_FILE]].concat()).await?;
    match output.status.code() {
        Some(0) => Ok(StateVolume::Populated),
        Some(1) => Ok(StateVolume::Empty),
        _ => Err(failed(&output)),
    }
}

async fn inspect(program: &str, name: &str) -> Result<StateVolume, Reason> {
    if !volume_exists(program, name).await? {
        return Ok(StateVolume::Empty);
    }
    volume_state(program, name).await
}

impl SidecarState for DockerVolumes {
    async fn volume(&self, node: &HostName) -> Result<StateVolume, Reason> {
        let name = sidecar_volume(node).map_err(|failure| Reason::new(&failure))?;
        inspect(&self.program, name.as_str()).await
    }
}

/// Brings the stacks below `directory` up with `docker compose`, decrypting their secrets with
/// the host's own age key exactly as the agent will.
#[derive(Debug, Clone)]
pub struct ComposeStacks {
    directory: PathBuf,
    program: String,
}

impl ComposeStacks {
    /// The stacks rendered below `directory`, run with `docker` from the `PATH`.
    #[must_use]
    pub fn new(directory: PathBuf) -> Self {
        Self::with_program("docker", directory)
    }

    /// The stacks rendered below `directory`, run with the `program` executable.
    #[must_use]
    pub fn with_program(program: impl Into<String>, directory: PathBuf) -> Self {
        Self {
            directory,
            program: program.into(),
        }
    }
}

fn environment_of(key: Option<&AuthKey>) -> Environment {
    let mut variables = BTreeMap::new();
    if let Some(key) = key {
        variables.insert(KEY_VARIABLE.to_owned(), key.expose().to_owned());
    }
    Environment::new(variables)
}

async fn request(
    directory: &Path,
    stack: &StackName,
    key: Option<&AuthKey>,
) -> Result<DeployRequest, Reason> {
    let relative =
        RepoPath::new(format!("aegis/{stack}")).map_err(|failure| Reason::new(&failure))?;
    let file = directory.join(relative.as_str()).join("compose.yaml");
    let text = tokio::fs::read_to_string(&file)
        .await
        .map_err(|failure| Reason::new(&failure.kind()))?;
    let summary = SerdeStackParser
        .parse_compose(&text)
        .map_err(|failure| Reason::new(&failure))?;
    let project = ProjectName::of(stack).map_err(|failure| Reason::new(&failure))?;
    let commit = CommitSha::new(NO_BUILD_COMMIT).map_err(|failure| Reason::new(&failure))?;
    let target = Target::new(project, relative);
    let extras = Extras::new(ExtraFiles::default(), environment_of(key)).with_sources(summary);
    Ok(DeployRequest::new(target, commit, BuiltServices::default()).with_extras(extras))
}

impl StackRunner for ComposeStacks {
    async fn start(&self, stack: &StackName, key: Option<&AuthKey>) -> Result<(), BootstrapError> {
        let deploy = request(&self.directory, stack, key).await;
        let deploy = deploy.map_err(BootstrapError::Run)?;
        let secrets = SecretPorts::new(SopsDecrypt::default(), TmpfsStore::from_environment());
        let compose = DecryptingCompose::new(
            DockerCompose::with_program(&self.program, &self.directory),
            secrets,
            &self.directory,
        );
        let done = compose.converge(&deploy).await;
        done.map_err(|failure| BootstrapError::Run(failure.reason().clone()))
    }
}
