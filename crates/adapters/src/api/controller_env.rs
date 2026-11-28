//! Wires a full controller from its environment.

use std::path::{Component, PathBuf};
use std::time::Duration;

use aegis_api::version_1::Role;
use aegis_application::{Poller, Repository};
use aegis_domain::HostName;
use tokio::net::TcpListener;

use super::agent::{invalid, required};
use super::controller::{AgentAddress, Hub};
use super::server::{Guard, LISTEN_VAR, ListenAddress, ServeError, listen_from, serve_routes};
use crate::git::Remote;
use crate::http::BaseUrl;
use crate::{FsRepo, GitFollower, SerdeStackParser, SopsCheck, TailscaleWhois};

const REPO_URL_VAR: &str = "AEGIS_REPO_URL";
const REPO_BRANCH_VAR: &str = "AEGIS_REPO_BRANCH";
const WORKDIR_VAR: &str = "AEGIS_WORKDIR";
const POLL_VAR: &str = "AEGIS_POLL_SECS";
const AGENTS_VAR: &str = "AEGIS_AGENTS";

/// The branch followed when `AEGIS_REPO_BRANCH` is not set.
const DEFAULT_BRANCH: &str = "main";

/// The seconds between two looks at the repository when `AEGIS_POLL_SECS` is not set.
const DEFAULT_POLL_SECS: u64 = 60;

/// The fewest seconds between two looks, so the remote is not hammered.
const MIN_POLL_SECS: u64 = 5;

/// The controller's settings, read from its environment.
struct Settings {
    listen: ListenAddress,
    remote: Remote,
    workdir: PathBuf,
    watch: Watch,
}

/// How often to look and which agents to tell.
struct Watch {
    interval: Duration,
    agents: Vec<AgentAddress>,
}

/// The `host=url` pairs of `AEGIS_AGENTS`, comma-separated; unset or empty means none.
fn agents_from(variable: Option<&str>) -> Result<Vec<AgentAddress>, ServeError> {
    let entries = variable.unwrap_or_default().split(',').map(str::trim);
    let present = entries.filter(|entry| !entry.is_empty());
    present.map(agent_from).collect()
}

fn agent_from(entry: &str) -> Result<AgentAddress, ServeError> {
    let (name, url) = entry
        .split_once('=')
        .ok_or_else(|| invalid(AGENTS_VAR, &"entries are host=url"))?;
    let host = HostName::new(name.trim()).map_err(|error| invalid(AGENTS_VAR, &error))?;
    Ok(AgentAddress::new(host, BaseUrl::new(url.trim())))
}

/// `AEGIS_POLL_SECS`, or the default; shorter than the minimum is refused.
fn interval_from(variable: Option<&str>) -> Result<Duration, ServeError> {
    let seconds = match variable {
        None => DEFAULT_POLL_SECS,
        Some(text) => text
            .trim()
            .parse()
            .map_err(|error| invalid(POLL_VAR, &error))?,
    };
    if seconds < MIN_POLL_SECS {
        return Err(invalid(POLL_VAR, &"must be at least 5 seconds"));
    }
    Ok(Duration::from_secs(seconds))
}

/// `AEGIS_WORKDIR`, which is wiped and rebuilt as needed, so it must be absolute.
fn workdir_from(value: &str) -> Result<PathBuf, ServeError> {
    let path = PathBuf::from(value);
    let climbs = path.components().any(|part| part == Component::ParentDir);
    let usable = path.is_absolute() && path.parent().is_some() && !climbs;
    usable.then_some(path).ok_or_else(|| {
        invalid(
            WORKDIR_VAR,
            &"must be an absolute path below the root, without ..",
        )
    })
}

impl Settings {
    fn from_env() -> Result<Self, ServeError> {
        let listen = listen_from(std::env::var(LISTEN_VAR))?;
        let branch = std::env::var(REPO_BRANCH_VAR).unwrap_or_else(|_| DEFAULT_BRANCH.to_owned());
        let remote = Remote::new(&required(REPO_URL_VAR)?, &branch)
            .map_err(|error| invalid(REPO_URL_VAR, &error))?;
        let watch = Watch {
            interval: interval_from(std::env::var(POLL_VAR).ok().as_deref())?,
            agents: agents_from(std::env::var(AGENTS_VAR).ok().as_deref())?,
        };
        Ok(Self {
            listen,
            remote,
            workdir: workdir_from(&required(WORKDIR_VAR)?)?,
            watch,
        })
    }

    fn poller(&self) -> Poller<GitFollower> {
        let git = GitFollower::new(&self.workdir, self.remote.clone());
        let repository = Repository::new(FsRepo::new(&self.workdir), SerdeStackParser, SopsCheck);
        Poller::new(git, repository)
    }
}

async fn run(settings: Settings) -> Result<(), ServeError> {
    let address = settings.listen.socket();
    let listener = TcpListener::bind(address)
        .await
        .map_err(|cause| ServeError::Bind { address, cause })?;
    let hub = Hub::new(&settings.watch.agents)?;
    let poller = settings.poller();
    let routes = hub.routes();
    let guard = Guard::new(Role::Controller, TailscaleWhois::new());
    let served = serve_routes(listener, guard, routes);
    tokio::select! {
        done = served => done,
        never = hub.run(&poller, settings.watch.interval) => match never {},
    }
}

/// Run a controller on its own runtime until it fails.
///
/// It listens on `AEGIS_LISTEN`, follows the `AEGIS_REPO_BRANCH` (default `main`) of the
/// repository at `AEGIS_REPO_URL` in the disposable checkout `AEGIS_WORKDIR`, looks every
/// `AEGIS_POLL_SECS` (default 60, at least 5) and tells the agents of `AEGIS_AGENTS`
/// (`host=url`, comma-separated) to converge. Git authenticates with what it inherits: a deploy
/// key through the SSH agent or a credential helper; a URL that carries credentials is refused.
///
/// # Errors
///
/// Returns a [`ServeError`] when a variable is missing or invalid, the runtime cannot start, the
/// address cannot be bound or the server fails.
pub fn serve_controller_from_env() -> Result<(), ServeError> {
    let settings = Settings::from_env()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(ServeError::Runtime)?;
    runtime.block_on(run(settings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_poll_interval_defaults_to_a_minute_and_has_a_floor() {
        assert_eq!(interval_from(None).unwrap(), Duration::from_secs(60));
        assert_eq!(interval_from(Some("30")).unwrap(), Duration::from_secs(30));
        assert_eq!(interval_from(Some("5")).unwrap(), Duration::from_secs(5));
        assert!(matches!(
            interval_from(Some("4")),
            Err(ServeError::Invalid { .. })
        ));
        assert!(matches!(
            interval_from(Some("soon")),
            Err(ServeError::Invalid { .. })
        ));
    }

    #[test]
    fn a_zero_or_empty_poll_interval_is_refused() {
        assert!(interval_from(Some("0")).is_err());
        assert!(interval_from(Some("")).is_err());
    }

    #[test]
    fn agents_are_comma_separated_host_url_pairs() {
        let agents =
            agents_from(Some("a=http://100.64.0.1:7480, b=http://100.64.0.2:7480,")).unwrap();
        assert_eq!(agents.len(), 2);
        assert!(agents_from(None).unwrap().is_empty());
        assert!(matches!(
            agents_from(Some("a")),
            Err(ServeError::Invalid { .. })
        ));
        assert!(matches!(
            agents_from(Some("../x=http://h")),
            Err(ServeError::Invalid { .. })
        ));
    }

    #[test]
    fn the_work_directory_must_be_absolute() {
        assert!(workdir_from("relative").is_err());
        assert!(workdir_from("/").is_err());
        assert!(workdir_from("/var/../etc").is_err());
        assert!(workdir_from("/var/lib/aegis/work").is_ok());
    }
}
