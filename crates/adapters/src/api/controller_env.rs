//! Wires a full controller from its environment.

use std::num::NonZeroU32;
use std::path::{Component, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use aegis_application::{Allowance, MintLimits, Poller, Repository};
use aegis_domain::{HostName, Logins};
use tokio::net::TcpListener;

use super::agent::{invalid, required};
use super::controller::{AgentAddress, Hub};
use super::enroll::{EnrollDesks, enroll_desks_from_env};
use super::server::{
    LISTEN_VAR, ListenAddress, Operated, ServeError, listen_from, serve_controller_for,
};
use crate::git::Remote;
use crate::http::BaseUrl;
use crate::{FsRepo, GitFollower, SerdeStackParser, SopsCheck, TailscaleWhois};

const REPO_URL_VAR: &str = "AEGIS_REPO_URL";
const REPO_BRANCH_VAR: &str = "AEGIS_REPO_BRANCH";
const WORKDIR_VAR: &str = "AEGIS_WORKDIR";
const POLL_VAR: &str = "AEGIS_POLL_SECS";
const AGENTS_VAR: &str = "AEGIS_AGENTS";
const OPERATORS_VAR: &str = "AEGIS_OPERATORS";
const MINT_PER_STACK_VAR: &str = "AEGIS_MINT_PER_STACK";
const MINT_PER_HOST_VAR: &str = "AEGIS_MINT_PER_HOST";
const MINT_TOTAL_VAR: &str = "AEGIS_MINT_TOTAL";
const MINT_WINDOW_VAR: &str = "AEGIS_MINT_WINDOW_SECS";

/// The fewest seconds of the mint window, so the limits cannot be made meaningless.
const MIN_MINT_WINDOW_SECS: u64 = 60;

/// The variables of the controller's own settings that it refuses to start without, besides
/// `AEGIS_LISTEN` and the enrolment variables of [`super::CONTROLLER_REQUIRED`].
pub const REPOSITORY_REQUIRED: [&str; 2] = [REPO_URL_VAR, WORKDIR_VAR];

/// The branch followed when `AEGIS_REPO_BRANCH` is not set.
const DEFAULT_BRANCH: &str = "main";

/// The seconds between two looks at the repository when `AEGIS_POLL_SECS` is not set.
const DEFAULT_POLL_SECS: u64 = 60;

/// The fewest seconds between two looks, so the remote is not hammered.
const MIN_POLL_SECS: u64 = 5;

/// The controller's settings, read from its environment.
struct Settings {
    serving: Serving,
    remote: Remote,
    workdir: PathBuf,
    watch: Watch,
}

/// Where the controller listens and the Tailscale side of the `POST /v1/enroll` and
/// `DELETE /v1/nodes/{hostname}` routes it serves beside the status.
struct Serving {
    listen: ListenAddress,
    enroll: EnrollDesks,
}

/// How often to look, which agents to tell, which people may operate the controller and how many
/// keys it mints.
struct Watch {
    interval: Duration,
    agents: Vec<AgentAddress>,
    operators: Logins,
    limits: MintLimits,
}

/// A count of keys from `variable`, or `standard` when it is unset; zero and anything that is no
/// whole number are refused.
fn allowance_from(
    name: &'static str,
    variable: Option<&str>,
    standard: Allowance,
) -> Result<Allowance, ServeError> {
    let Some(text) = variable else {
        return Ok(standard);
    };
    let count: NonZeroU32 = text.trim().parse().map_err(|error| invalid(name, &error))?;
    Ok(Allowance::new(count))
}

/// The key limits of the environment: `AEGIS_MINT_PER_STACK`, `AEGIS_MINT_PER_HOST`,
/// `AEGIS_MINT_TOTAL` and `AEGIS_MINT_WINDOW_SECS`, each defaulting to the standard limits.
fn limits_from_env() -> Result<MintLimits, ServeError> {
    let standard = MintLimits::standard();
    let read = |name: &'static str| std::env::var(name).ok();
    let per_stack = allowance_from(
        MINT_PER_STACK_VAR,
        read(MINT_PER_STACK_VAR).as_deref(),
        standard.per_stack(),
    )?;
    let per_host = allowance_from(
        MINT_PER_HOST_VAR,
        read(MINT_PER_HOST_VAR).as_deref(),
        standard.per_host(),
    )?;
    let total = allowance_from(
        MINT_TOTAL_VAR,
        read(MINT_TOTAL_VAR).as_deref(),
        standard.total(),
    )?;
    let window = window_from(read(MINT_WINDOW_VAR).as_deref(), standard.window())?;
    Ok(MintLimits::new(per_stack, per_host, total).over(window))
}

/// `AEGIS_MINT_WINDOW_SECS`, or `standard`; shorter than the minimum is refused.
fn window_from(variable: Option<&str>, standard: Duration) -> Result<Duration, ServeError> {
    let Some(text) = variable else {
        return Ok(standard);
    };
    let seconds: u64 = text
        .trim()
        .parse()
        .map_err(|error| invalid(MINT_WINDOW_VAR, &error))?;
    if seconds < MIN_MINT_WINDOW_SECS {
        return Err(invalid(MINT_WINDOW_VAR, &"must be at least 60 seconds"));
    }
    Ok(Duration::from_secs(seconds))
}

/// The logins of `AEGIS_OPERATORS`, comma-separated; unset or empty means nobody, so no operator
/// is admitted.
fn operators_from(variable: Option<&str>) -> Result<Logins, ServeError> {
    Logins::parse_list(variable.unwrap_or_default()).map_err(|error| invalid(OPERATORS_VAR, &error))
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
    let carries_credentials = url.contains('@');
    carries_credentials.then_some(()).map_or(Ok(()), |()| {
        Err(invalid(AGENTS_VAR, &"URLs carry no credentials"))
    })?;
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
            operators: operators_from(std::env::var(OPERATORS_VAR).ok().as_deref())?,
            limits: limits_from_env()?,
        };
        let serving = Serving {
            listen,
            enroll: enroll_desks_from_env()?,
        };
        Ok(Self {
            serving,
            remote,
            workdir: workdir_from(&required(WORKDIR_VAR)?)?,
            watch,
        })
    }

    fn poller(&self) -> Arc<Poller<GitFollower>> {
        let git = GitFollower::new(&self.workdir, self.remote.clone());
        let repository = Repository::new(FsRepo::new(&self.workdir), SerdeStackParser, SopsCheck);
        Arc::new(Poller::new(git, repository))
    }
}

async fn run(settings: Settings) -> Result<(), ServeError> {
    let address = settings.serving.listen.socket();
    let listener = TcpListener::bind(address)
        .await
        .map_err(|cause| ServeError::Bind { address, cause })?;
    let hub = Hub::limited(&settings.watch.agents, settings.watch.limits)?;
    let poller = settings.poller();
    let enroll = settings.serving.enroll.routes(&hub.binding());
    let routes = hub.grouped(enroll, Arc::clone(&poller));
    let operators = settings.watch.operators.clone();
    let served = serve_controller_for(
        listener,
        Operated::new(TailscaleWhois::new(), operators),
        routes,
    );
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
/// (`host=url`, comma-separated) to converge. It also serves `POST /v1/enroll`, which needs the
/// Tailscale variables of `AEGIS_TS_CLIENT_ID`, `AEGIS_TS_CLIENT_SECRET` and `AEGIS_ENROLL_TAGS`. Git authenticates with what it inherits: a deploy
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
    fn an_agent_url_with_an_at_sign_anywhere_is_refused() {
        assert!(agents_from(Some("a=http://user:pw@h")).is_err());
        assert!(agents_from(Some("a=http://token@h")).is_err());
        assert!(agents_from(Some("a=http://h/path@x")).is_err());
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
    fn operators_are_comma_separated_logins_and_nobody_by_default() {
        assert!(operators_from(None).unwrap().is_empty());
        assert!(operators_from(Some(" , ")).unwrap().is_empty());
        let listed = operators_from(Some("Ada@Example.com, bob@example.com")).unwrap();
        assert_eq!(listed.len(), 2);
        assert!(matches!(
            operators_from(Some("ada lovelace")),
            Err(ServeError::Invalid { .. })
        ));
    }

    #[test]
    fn the_work_directory_must_be_absolute() {
        assert!(workdir_from("relative").is_err());
        assert!(workdir_from("/").is_err());
        assert!(workdir_from("/var/../etc").is_err());
        assert!(workdir_from("/var/lib/..").is_err());
        assert!(workdir_from("/var/lib/aegis/work").is_ok());
    }

    #[test]
    fn a_mint_allowance_defaults_and_refuses_zero_and_words() {
        let standard = MintLimits::standard().total();
        let unset = allowance_from(MINT_TOTAL_VAR, None, standard).unwrap();
        assert_eq!(unset, standard);
        let set = allowance_from(MINT_TOTAL_VAR, Some(" 7 "), standard).unwrap();
        assert_ne!(set, standard);
        assert!(allowance_from(MINT_TOTAL_VAR, Some("0"), standard).is_err());
        assert!(allowance_from(MINT_TOTAL_VAR, Some("many"), standard).is_err());
    }

    #[test]
    fn the_mint_window_has_a_floor() {
        let standard = Duration::from_hours(1);
        assert_eq!(window_from(None, standard).unwrap(), standard);
        let minute = window_from(Some("60"), standard).unwrap();
        assert_eq!(minute, Duration::from_mins(1));
        assert!(window_from(Some("59"), standard).is_err());
        assert!(window_from(Some("soon"), standard).is_err());
    }
}
