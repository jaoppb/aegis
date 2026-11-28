//! The Docker adapters of removal: finds the compose projects Aegis left behind, and deletes the
//! data of a purged one.
//!
//! Both read the labels Compose puts on the containers and volumes it creates, so nothing about
//! what ran has to be stored: the host stays disposable.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use aegis_application::{Projects, Purger, Reason, SafePath, TAILSCALE_IMAGE};
use aegis_domain::{Detail, Leftover, Leftovers, ProjectName, Remains, Tombstone};
use tokio::process::Command;

use crate::cli;

/// How long one Docker query or delete may take.
const DOCKER_TIMEOUT: Duration = Duration::from_mins(5);

/// The label Compose puts on everything a project creates.
const PROJECT_LABEL: &str = "com.docker.compose.project";

/// The label of the directory Compose was run from.
const DIRECTORY_LABEL: &str = "com.docker.compose.project.working_dir";

/// The run of `docker`, ready to be started in the sandbox the adapters share.
fn docker(program: &str) -> Command {
    let mut command = Command::new(program);
    cli::inherit_only(&mut command, &cli::COMPOSE_INHERITED);
    command
}

fn failure(output: &Output) -> Reason {
    let stderr = String::from_utf8_lossy(&output.stderr);
    Reason::new(&format!(
        "docker exited with {}: {}",
        output.status,
        stderr.trim()
    ))
}

async fn succeed(command: Command) -> Result<Output, Reason> {
    let output = cli::run(command, String::new(), DOCKER_TIMEOUT).await?;
    output
        .status
        .success()
        .then_some(output.clone())
        .ok_or_else(|| failure(&output))
}

/// One container of a compose project.
struct Container {
    identifier: String,
    project: String,
    directory: String,
}

fn parse_containers(text: &str) -> Vec<Container> {
    let rows = text.lines().map(|line| line.splitn(3, '\t'));
    let fields = rows.map(|mut parts| (parts.next(), parts.next(), parts.next()));
    fields
        .filter_map(|(identifier, project, directory)| {
            Some(Container {
                identifier: identifier?.to_owned(),
                project: project?.to_owned(),
                directory: directory?.to_owned(),
            })
        })
        .collect()
}

/// A volume and the project it belongs to.
struct Labelled {
    volume: String,
    project: String,
}

fn parse_volumes(text: &str) -> Vec<Labelled> {
    let rows = text.lines().filter_map(|line| line.split_once('\t'));
    rows.map(|(volume, project)| Labelled {
        volume: volume.to_owned(),
        project: project.to_owned(),
    })
    .collect()
}

/// What counts as this host's: containers started from under `scope`, and not the agent's own.
#[derive(Debug, Clone)]
struct Scope {
    directory: PathBuf,
    own_container: Option<String>,
}

impl Scope {
    fn holds(&self, container: &Container) -> bool {
        let inside = Path::new(&container.directory).starts_with(&self.directory);
        inside && !self.is_own(&container.identifier)
    }

    fn is_own(&self, identifier: &str) -> bool {
        let same = |own: &String| own.starts_with(identifier) || identifier.starts_with(own);
        !identifier.is_empty() && self.own_container.as_ref().is_some_and(same)
    }
}

/// Reads the projects of the host from the labels on its containers and volumes.
#[derive(Debug, Clone)]
pub struct DockerProjects {
    program: String,
    scope: Scope,
}

impl DockerProjects {
    /// Projects started from under `directory`, the host's directory in the checkout, with the
    /// `docker` from the `PATH`.
    #[must_use]
    pub fn new<Directory: Into<PathBuf>>(directory: Directory) -> Self {
        Self::with_program("docker", directory)
    }

    /// The same with the `program` executable.
    #[must_use]
    pub fn with_program<Program: Into<String>, Directory: Into<PathBuf>>(
        program: Program,
        directory: Directory,
    ) -> Self {
        Self {
            program: program.into(),
            scope: Scope {
                directory: directory.into(),
                own_container: None,
            },
        }
    }

    /// The same projects, never counting the container `identifier` or its project: it is the
    /// agent itself. An empty `identifier` excludes nothing.
    #[must_use]
    pub fn excluding(self, identifier: &str) -> Self {
        let own = (!identifier.is_empty()).then(|| identifier.to_owned());
        let scope = Scope {
            own_container: own,
            ..self.scope
        };
        Self { scope, ..self }
    }

    async fn containers(&self) -> Result<Vec<Container>, Reason> {
        let format = format!(
            "{{{{.ID}}}}\t{{{{.Label \"{PROJECT_LABEL}\"}}}}\t{{{{.Label \"{DIRECTORY_LABEL}\"}}}}"
        );
        let mut command = docker(&self.program);
        command.args([
            "ps",
            "-a",
            "--no-trunc",
            "--filter",
            &format!("label={PROJECT_LABEL}"),
        ]);
        command.args(["--format", &format]);
        let output = succeed(command).await?;
        Ok(parse_containers(&String::from_utf8_lossy(&output.stdout)))
    }

    async fn volumes(&self) -> Result<Vec<Labelled>, Reason> {
        let format = format!("{{{{.Name}}}}\t{{{{.Label \"{PROJECT_LABEL}\"}}}}");
        let mut command = docker(&self.program);
        command.args([
            "volume",
            "ls",
            "--filter",
            &format!("label={PROJECT_LABEL}"),
        ]);
        command.args(["--format", &format]);
        let output = succeed(command).await?;
        Ok(parse_volumes(&String::from_utf8_lossy(&output.stdout)))
    }
}

/// The projects with a container this host does not own, so they are not its to take down.
fn foreign(containers: &[Container], scope: &Scope) -> BTreeSet<String> {
    let outside = containers
        .iter()
        .filter(|container| !scope.holds(container));
    outside.map(|container| container.project.clone()).collect()
}

fn leftovers_of(containers: &[Container], volumes: &[Labelled], scope: &Scope) -> Leftovers {
    let foreign = foreign(containers, scope);
    let mut found: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for container in containers {
        found.entry(container.project.as_str()).or_default();
    }
    for labelled in volumes {
        let kept = found.entry(labelled.project.as_str()).or_default();
        kept.push(labelled.volume.as_str());
    }
    let owned = found
        .into_iter()
        .filter(|(project, _)| !foreign.contains(*project));
    Leftovers::new(
        owned
            .filter_map(|(project, kept)| leftover(project, &kept))
            .collect(),
    )
}

fn leftover(project: &str, volumes: &[&str]) -> Option<Leftover> {
    let name = ProjectName::parse(project)?;
    let lines = volumes
        .iter()
        .map(|volume| Detail::new(&format!("volume {volume}")));
    Some(Leftover::new(name, Remains::new(lines.collect())))
}

impl Projects for DockerProjects {
    async fn leftovers(&self) -> Result<Leftovers, Reason> {
        let containers = self.containers().await?;
        let volumes = self.volumes().await?;
        Ok(leftovers_of(&containers, &volumes, &self.scope))
    }
}

/// Deletes the data of a purged stack with the `docker` CLI: its labelled volumes, and its host
/// paths through a throw-away container, which also reaches files a rootless daemon owns.
#[derive(Debug, Clone)]
pub struct DockerPurger {
    program: String,
}

impl Default for DockerPurger {
    fn default() -> Self {
        Self::with_program("docker")
    }
}

/// The shell that deletes the entry `$1` of the mounted parent directory, refusing a symlink so
/// the delete can never follow a link out of the path it was checked against.
const REMOVE_SCRIPT: &str = "test ! -L \"/parent/$1\" && rm -rf -- \"/parent/$1\"";

impl DockerPurger {
    /// A purger that runs the `program` executable.
    #[must_use]
    pub fn with_program<Program: Into<String>>(program: Program) -> Self {
        Self {
            program: program.into(),
        }
    }

    async fn labelled_volumes(&self, project: &ProjectName) -> Result<Vec<String>, Reason> {
        let mut command = docker(&self.program);
        command.args(["volume", "ls", "--quiet", "--filter"]);
        command.arg(format!("label={PROJECT_LABEL}={project}"));
        let output = succeed(command).await?;
        let text = String::from_utf8_lossy(&output.stdout);
        Ok(text.lines().map(str::to_owned).collect())
    }
}

fn plain_volume(name: &str) -> Result<(), Reason> {
    let bad = name.is_empty() || name.starts_with('-');
    (!bad)
        .then_some(())
        .ok_or_else(|| Reason::new(&"a volume has an unusable name"))
}

async fn remove_named(program: &str, names: &[String]) -> Result<(), Reason> {
    if names.is_empty() {
        return Ok(());
    }
    let mut command = docker(program);
    command.args(["volume", "rm"]).args(names);
    succeed(command).await.map(drop)
}

/// The directory that holds the last segment of `path`, and that segment.
struct Entry<'path> {
    parent: &'path str,
    name: &'path str,
}

fn entry_of(path: &str) -> Result<Entry<'_>, Reason> {
    let split = path.rsplit_once('/');
    let usable = split.filter(|found| !found.0.is_empty() && !found.1.is_empty());
    let found = usable.ok_or_else(|| Reason::new(&"the path has no parent directory"))?;
    Ok(Entry {
        parent: found.0,
        name: found.1,
    })
}

impl Purger for DockerPurger {
    async fn remove_volumes(&self, tombstone: &Tombstone) -> Result<(), Reason> {
        let names = self.labelled_volumes(tombstone.project()).await?;
        names.iter().try_for_each(|name| plain_volume(name))?;
        remove_named(&self.program, &names).await
    }

    async fn remove_path(&self, _tombstone: &Tombstone, path: &SafePath) -> Result<(), Reason> {
        let entry = entry_of(path.path().as_str())?;
        let mut command = docker(&self.program);
        command.args(["run", "--rm", "--network", "none", "--entrypoint", "sh"]);
        command.args(["-v", &format!("{}:/parent", entry.parent), TAILSCALE_IMAGE]);
        command.args(["-c", REMOVE_SCRIPT, "sh", entry.name]);
        succeed(command).await.map(drop)
    }
}
