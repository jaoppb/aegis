//! The agent's side of the API: `POST /v1/reconcile` and the wiring of a full agent from its
//! environment.

use std::path::PathBuf;
use std::sync::Arc;

use aegis_api::version_1::{RECONCILE_PATH, ReconcileRequest, Role, StatusReport};
use aegis_application::{Reconcile, ReconcilePorts, Reconciler, Repository, SecretStore, WhoIs};
use aegis_domain::{CommitSha, HostName, ReservedStacks, StackName};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use super::server::{Guard, LISTEN_VAR, ListenAddress, ServeError, listen_from, serve_routes};
use super::wire::status_report;
use super::{ApiClient, BaseUrl};
use crate::{
    DecryptingCompose, DockerCompose, FsRepo, GitCheckout, SecretPorts, SerdeStackParser,
    SopsCheck, SopsDecrypt, TailscaleWhois, TmpfsStore,
};

/// The compose runner of the agent: `docker compose` behind the per-host SOPS decryption.
type Compose = DecryptingCompose<DockerCompose, SopsDecrypt, TmpfsStore>;

const HOST_VAR: &str = "AEGIS_HOST";
const CHECKOUT_VAR: &str = "AEGIS_CHECKOUT";
const CONTROLLER_VAR: &str = "AEGIS_CONTROLLER";
const RESERVED_VAR: &str = "AEGIS_RESERVED";

/// What a reconcile handler needs: the reconciler, where to report, and a lock that lets one
/// reconcile at a time touch the checkout.
struct Agent<Run> {
    reconciler: Run,
    controller: ApiClient,
    turn: Mutex<()>,
}

/// The `POST /v1/reconcile` route: converge the host to the commit in the body, report to
/// `controller` and answer with the same report. One reconcile runs at a time; the others wait.
pub fn reconcile_routes<Run>(reconciler: Run, controller: ApiClient) -> Router
where
    Run: Reconcile + Send + Sync + 'static,
{
    let agent = Agent {
        reconciler,
        controller,
        turn: Mutex::new(()),
    };
    Router::new()
        .route(RECONCILE_PATH, post(reconcile::<Run>))
        .with_state(Arc::new(agent))
}

/// A refused reconcile: a status and a short plain-text reason that holds no secrets.
struct Refusal {
    status: StatusCode,
    reason: String,
}

impl Refusal {
    fn new(status: StatusCode, reason: &impl std::fmt::Display) -> Self {
        Self {
            status,
            reason: reason.to_string(),
        }
    }
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        (self.status, self.reason).into_response()
    }
}

async fn reconcile<Run>(
    State(agent): State<Arc<Agent<Run>>>,
    Json(request): Json<ReconcileRequest>,
) -> Result<Json<StatusReport>, Refusal>
where
    Run: Reconcile + Send + Sync + 'static,
{
    let commit = CommitSha::new(request.commit())
        .map_err(|error| Refusal::new(StatusCode::BAD_REQUEST, &error))?;
    // The reconcile runs in its own task, so dropping this handler (a transport that cancels on
    // hang-up, or a timeout layer added later) cannot stop it half way and skip removing the
    // decrypted files. The HTTP/1 server used today keeps running the handler anyway.
    let worker = Arc::clone(&agent);
    let task = tokio::spawn(async move {
        let _turn = worker.turn.lock().await;
        worker.reconciler.reconcile(commit).await
    });
    let done = task.await.map_err(|_| {
        Refusal::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            &"the reconcile task failed",
        )
    })?;
    let report =
        status_report(&done.map_err(|error| Refusal::new(StatusCode::BAD_GATEWAY, &error))?);
    // Reporting is best effort: the controller also gets the report as the answer.
    agent.controller.report_status(&report).await.ok();
    Ok(Json(report))
}

/// The agent's settings, read from its environment.
struct Settings {
    listen: ListenAddress,
    hosting: Hosting,
    checkout: PathBuf,
    controller: BaseUrl,
}

/// Which stacks the agent deploys: its host's, and the reserved ones it opted in to.
struct Hosting {
    host: HostName,
    reserved: ReservedStacks,
}

fn required(name: &'static str) -> Result<String, ServeError> {
    std::env::var(name).map_err(|_| ServeError::Missing(name))
}

fn invalid(variable: &'static str, cause: &impl std::fmt::Display) -> ServeError {
    ServeError::Invalid {
        variable,
        cause: cause.to_string(),
    }
}

/// The comma-separated stack names of `AEGIS_RESERVED`; unset or empty means none.
fn reserved_from(variable: Option<&str>) -> Result<ReservedStacks, ServeError> {
    let names = variable.unwrap_or_default().split(',').map(str::trim);
    let present = names.filter(|name| !name.is_empty());
    present
        .map(|name| StackName::new(name).map_err(|error| invalid(RESERVED_VAR, &error)))
        .collect::<Result<Vec<_>, _>>()
        .map(ReservedStacks::new)
}

/// `AEGIS_CHECKOUT`, which the secret paths and Docker bind mounts are built from, so it must be
/// absolute.
fn absolute_checkout(value: &str) -> Result<PathBuf, ServeError> {
    let path = PathBuf::from(value);
    path.is_absolute()
        .then_some(path)
        .ok_or_else(|| invalid(CHECKOUT_VAR, &"must be an absolute path"))
}

impl Settings {
    fn from_env() -> Result<Self, ServeError> {
        let listen = listen_from(std::env::var(LISTEN_VAR))?;
        let host = HostName::new(required(HOST_VAR)?).map_err(|error| invalid(HOST_VAR, &error))?;
        Ok(Self {
            listen,
            hosting: Hosting {
                host,
                reserved: reserved_from(std::env::var(RESERVED_VAR).ok().as_deref())?,
            },
            checkout: absolute_checkout(&required(CHECKOUT_VAR)?)?,
            controller: BaseUrl::new(required(CONTROLLER_VAR)?),
        })
    }

    fn compose(&self) -> Compose {
        let secrets = SecretPorts::new(SopsDecrypt::default(), TmpfsStore::from_environment());
        DecryptingCompose::new(DockerCompose::new(&self.checkout), secrets, &self.checkout)
    }

    fn reconciler(&self) -> Reconciler<GitCheckout, Compose> {
        let repository = Repository::new(FsRepo::new(&self.checkout), SerdeStackParser, SopsCheck);
        let git = GitCheckout::new(&self.checkout);
        let ports = ReconcilePorts::new(git, self.compose(), repository);
        Reconciler::new(self.hosting.host.clone(), ports)
            .with_reserved(self.hosting.reserved.clone())
    }
}

/// Serve an agent on `listener`: the health endpoint and the extra `routes`, such as
/// [`reconcile_routes`], behind the tag check that admits only the controller.
///
/// # Errors
///
/// The same as [`super::serve`].
pub async fn serve_agent<Lookup>(
    listener: TcpListener,
    whois: Lookup,
    routes: Router,
) -> Result<(), ServeError>
where
    Lookup: WhoIs + Send + Sync + 'static,
{
    serve_routes(listener, Guard::new(Role::Agent, whois), routes).await
}

async fn run(settings: Settings) -> Result<(), ServeError> {
    let address = settings.listen.socket();
    let listener = TcpListener::bind(address)
        .await
        .map_err(|cause| ServeError::Bind { address, cause })?;
    let controller = ApiClient::new(settings.controller.clone())?;
    let swept = TmpfsStore::from_environment().sweep().await;
    swept.map_err(ServeError::Sweep)?;
    let routes = reconcile_routes(settings.reconciler(), controller);
    serve_agent(listener, TailscaleWhois::new(), routes).await
}

/// Run an agent on its own runtime until it fails.
///
/// It listens on `AEGIS_LISTEN`, deploys the stacks of `AEGIS_HOST` from the git checkout at
/// `AEGIS_CHECKOUT`, and reports to the controller at `AEGIS_CONTROLLER`. The reserved `aegis/`
/// stacks named in `AEGIS_RESERVED` (comma-separated, optional) are deployed too.
///
/// # Errors
///
/// Returns a [`ServeError`] when a variable is missing or invalid, the runtime cannot start, the
/// address cannot be bound, decrypted files left by an earlier run cannot be removed or the server
/// fails.
pub fn serve_agent_from_env() -> Result<(), ServeError> {
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
    fn reserved_stacks_are_a_comma_separated_list() {
        let reserved = reserved_from(Some("controller, agent,")).unwrap();
        let names: Vec<&str> = reserved.iter().map(StackName::as_str).collect();
        assert_eq!(names, ["controller", "agent"]);
    }

    #[test]
    fn no_reserved_stacks_is_the_default() {
        assert!(reserved_from(None).unwrap().is_empty());
        assert!(reserved_from(Some("")).unwrap().is_empty());
    }

    #[test]
    fn the_checkout_must_be_an_absolute_path() {
        assert!(matches!(
            absolute_checkout("relative/repo"),
            Err(ServeError::Invalid { .. })
        ));
        assert_eq!(
            absolute_checkout("/var/lib/aegis/repo").unwrap(),
            PathBuf::from("/var/lib/aegis/repo")
        );
    }

    #[test]
    fn a_bad_reserved_name_is_refused() {
        let result = reserved_from(Some("../etc"));
        assert!(matches!(result, Err(ServeError::Invalid { .. })));
    }
}
