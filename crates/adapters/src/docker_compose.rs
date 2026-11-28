//! The `docker compose` CLI adapter of the [`ComposeRunner`] port.
//!
//! Every deploy pins the stack's own `compose.yaml` with `-f`, so Compose never merges a
//! `compose.override.yaml` or reads `compose.yml` or `docker-compose.yml` from the stack directory.

use std::path::PathBuf;
use std::process::Output;
use std::time::Duration;

use aegis_application::{ComposeError, ComposeRunner, DeployRequest, Environment, Reason};
use tokio::process::Command;

use crate::cli;

/// How long one deploy may take; builds are slow.
const COMPOSE_TIMEOUT: Duration = Duration::from_mins(30);

/// The one compose file a stack is deployed from.
const COMPOSE_FILE: &str = "compose.yaml";

/// The environment Compose inherits from the agent, and nothing else, so `COMPOSE_FILE`,
/// `COMPOSE_PROJECT_NAME` and the like in the agent's environment cannot change what runs.
const INHERITED: [&str; 9] = [
    "PATH",
    "HOME",
    "DOCKER_HOST",
    "DOCKER_CONTEXT",
    "DOCKER_CONFIG",
    "DOCKER_CERT_PATH",
    "DOCKER_TLS_VERIFY",
    "XDG_RUNTIME_DIR",
    "SSL_CERT_FILE",
];

/// Runs `docker compose` against the local Docker daemon, rootless or rootful: the socket comes
/// from the inherited `DOCKER_HOST`.
#[derive(Debug, Clone)]
pub struct DockerCompose {
    program: String,
    checkout: PathBuf,
}

impl DockerCompose {
    /// Deploy stacks of the checkout at `checkout` with `docker` from the `PATH`.
    #[must_use]
    pub fn new<Checkout: Into<PathBuf>>(checkout: Checkout) -> Self {
        Self::with_program("docker", checkout)
    }

    /// Deploy stacks of the checkout at `checkout` with the `program` executable.
    #[must_use]
    pub fn with_program<Program: Into<String>, Checkout: Into<PathBuf>>(
        program: Program,
        checkout: Checkout,
    ) -> Self {
        Self {
            program: program.into(),
            checkout: checkout.into(),
        }
    }

    fn command(&self, request: &DeployRequest) -> Command {
        let directory = self.checkout.join(request.target().directory().as_str());
        let mut command = Command::new(&self.program);
        command.current_dir(directory);
        cli::inherit_only(&mut command, &INHERITED);
        command.envs(request.extras().environment().variables());
        command.args(arguments(request));
        command
    }
}

/// The arguments: `compose -p <project> -f compose.yaml [-f <extra>]... [-f -] up -d --build`.
/// The `-f -` override that tags built images reads its YAML from stdin.
fn arguments(request: &DeployRequest) -> Vec<String> {
    let project = request.target().project().to_string();
    let mut arguments = vec!["compose".to_owned(), "-p".to_owned(), project];
    arguments.extend(["-f".to_owned(), COMPOSE_FILE.to_owned()]);
    for file in request.extras().files().as_slice() {
        arguments.extend(["-f".to_owned(), file.to_string()]);
    }
    if !request.built().as_slice().is_empty() {
        arguments.extend(["-f".to_owned(), "-".to_owned()]);
    }
    arguments.extend(["up", "-d", "--build"].map(str::to_owned));
    arguments
}

/// The override that adds a commit tag to every service built from the repository.
fn build_tags(request: &DeployRequest) -> String {
    let project = request.target().project();
    let commit = request.commit();
    let services = request.built().as_slice().iter();
    let entries = services.map(|service| {
        let image = format!(
            "{project}-{}:{commit}",
            service.as_str().to_ascii_lowercase()
        );
        format!("  {service}:\n    build:\n      tags:\n        - \"{image}\"\n")
    });
    format!("services:\n{}", entries.collect::<String>())
}

/// Redact every injected value from `text`.
fn redact(text: &str, environment: &Environment) -> String {
    let values = environment.variables().values();
    let secret = values.filter(|value| !value.is_empty());
    secret.fold(text.to_owned(), |clean, value| clean.replace(value, "***"))
}

fn failure(output: &Output, environment: &Environment) -> Reason {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let message = redact(&stderr, environment);
    Reason::new(&format!(
        "docker exited with {}: {}",
        output.status,
        message.trim()
    ))
}

async fn run(command: Command, stdin: String, request: &DeployRequest) -> Result<(), Reason> {
    let output = cli::run(command, stdin, COMPOSE_TIMEOUT).await?;
    if output.status.success() {
        return Ok(());
    }
    Err(failure(&output, request.extras().environment()))
}

impl ComposeRunner for DockerCompose {
    async fn converge(&self, request: &DeployRequest) -> Result<(), ComposeError> {
        let command = self.command(request);
        let input = build_tags(request);
        run(command, input, request)
            .await
            .map_err(ComposeError::new)
    }
}
