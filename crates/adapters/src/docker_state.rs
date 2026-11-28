//! The `docker run` adapter of the [`StateProbe`] port: looks for the node identity in a
//! project's sidecar state volume.

use std::time::Duration;

use aegis_application::{
    Reason, STATE_DIRECTORY, STATE_FILE, StateProbe, StateVolume, TAILSCALE_IMAGE, state_volume_of,
};
use aegis_domain::ProjectName;
use tokio::process::Command;

use crate::cli;

/// How long the probe may take; it may have to pull the sidecar image first.
const PROBE_TIMEOUT: Duration = Duration::from_mins(5);

/// What `test -s` exits with when the file is missing or empty.
const NOT_FOUND: i32 = 1;

/// Inspects the state volume with a throw-away container, rootless or rootful: the socket comes
/// from the inherited `DOCKER_HOST`. The container has no network and mounts the volume read-only.
#[derive(Debug, Clone)]
pub struct DockerStateProbe {
    program: String,
}

impl Default for DockerStateProbe {
    fn default() -> Self {
        Self::with_program("docker")
    }
}

impl DockerStateProbe {
    /// A probe that runs the `program` executable.
    #[must_use]
    pub fn with_program<Program: Into<String>>(program: Program) -> Self {
        Self {
            program: program.into(),
        }
    }

    fn command(&self, project: &ProjectName) -> Command {
        let volume = format!("{}:{STATE_DIRECTORY}:ro", state_volume_of(project));
        let file = format!("{STATE_DIRECTORY}/{STATE_FILE}");
        let mut command = Command::new(&self.program);
        cli::inherit_only(&mut command, &cli::COMPOSE_INHERITED);
        command.args(["run", "--rm", "--network", "none", "--entrypoint", "test"]);
        command.args(["-v", &volume, TAILSCALE_IMAGE, "-s", &file]);
        command
    }
}

impl StateProbe for DockerStateProbe {
    async fn probe(&self, project: &ProjectName) -> Result<StateVolume, Reason> {
        let output = cli::run(self.command(project), String::new(), PROBE_TIMEOUT).await?;
        match output.status.code() {
            Some(0) => Ok(StateVolume::Populated),
            Some(NOT_FOUND) => Ok(StateVolume::Empty),
            other => Err(Reason::new(&format!(
                "the state check exited with {}",
                other.map_or_else(|| "a signal".to_owned(), |code| code.to_string())
            ))),
        }
    }
}
