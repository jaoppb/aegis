//! The compose runner of bootstrap for the stacks rendered below the bootstrap directory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use aegis_application::{
    AuthKey, BootstrapError, BuiltServices, ComposeRunner, DeployRequest, Environment, ExtraFiles,
    Extras, KEY_VARIABLE, Reason, StackParser, StackRunner, Target,
};
use aegis_domain::{CommitSha, ProjectName, RepoPath, StackName};

use crate::{
    DecryptingCompose, DockerCompose, SecretPorts, SerdeStackParser, SopsDecrypt, TmpfsStore,
};

/// The commit a bootstrap deploy names. No service is built, so no image is tagged with it.
const NO_BUILD_COMMIT: &str = "0000000";

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
