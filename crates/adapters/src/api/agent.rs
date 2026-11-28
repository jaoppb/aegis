//! The agent's side of the API: `POST /v1/reconcile` and the wiring of a full agent from its
//! environment.

use std::path::PathBuf;
use std::sync::Arc;

use aegis_api::version_1::{RECONCILE_PATH, ReconcileRequest, Role, StatusReport};
use aegis_application::{Ports, Reconcile, Reconciler, Repository, WhoIs};
use aegis_domain::{CommitSha, HostName};
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use super::server::{Guard, LISTEN_VAR, ListenAddress, ServeError, listen_from, serve_routes};
use super::wire::status_report;
use super::{ApiClient, BaseUrl};
use crate::{DockerCompose, FsRepo, GitCheckout, SerdeStackParser, SopsCheck, TailscaleWhois};

const HOST_VAR: &str = "AEGIS_HOST";
const CHECKOUT_VAR: &str = "AEGIS_CHECKOUT";
const CONTROLLER_VAR: &str = "AEGIS_CONTROLLER";

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

async fn reconcile<Run>(
    State(agent): State<Arc<Agent<Run>>>,
    Json(request): Json<ReconcileRequest>,
) -> Result<Json<StatusReport>, StatusCode>
where
    Run: Reconcile + Send + Sync + 'static,
{
    let commit = CommitSha::new(request.commit()).map_err(|_| StatusCode::BAD_REQUEST)?;
    let _turn = agent.turn.lock().await;
    let done = agent.reconciler.reconcile(commit).await;
    let report = status_report(&done.map_err(|_| StatusCode::BAD_GATEWAY)?);
    // Reporting is best effort: the controller also gets the report as the answer.
    agent.controller.report_status(&report).await.ok();
    Ok(Json(report))
}

/// The agent's settings, read from its environment.
struct Settings {
    listen: ListenAddress,
    host: HostName,
    checkout: PathBuf,
    controller: BaseUrl,
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

impl Settings {
    fn from_env() -> Result<Self, ServeError> {
        let listen = listen_from(std::env::var(LISTEN_VAR))?;
        let host = HostName::new(required(HOST_VAR)?).map_err(|error| invalid(HOST_VAR, &error))?;
        Ok(Self {
            listen,
            host,
            checkout: PathBuf::from(required(CHECKOUT_VAR)?),
            controller: BaseUrl::new(required(CONTROLLER_VAR)?),
        })
    }

    fn reconciler(&self) -> Reconciler<GitCheckout, DockerCompose> {
        let repository = Repository::new(FsRepo::new(&self.checkout), SerdeStackParser, SopsCheck);
        let git = GitCheckout::new(&self.checkout);
        let ports = Ports::new(git, DockerCompose::new(&self.checkout), repository);
        Reconciler::new(self.host.clone(), ports)
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
    let routes = reconcile_routes(settings.reconciler(), controller);
    serve_agent(listener, TailscaleWhois::new(), routes).await
}

/// Run an agent on its own runtime until it fails.
///
/// It listens on `AEGIS_LISTEN`, deploys the stacks of `AEGIS_HOST` from the git checkout at
/// `AEGIS_CHECKOUT`, and reports to the controller at `AEGIS_CONTROLLER`.
///
/// # Errors
///
/// Returns a [`ServeError`] when a variable is missing or invalid, the runtime cannot start, the
/// address cannot be bound or the server fails.
pub fn serve_agent_from_env() -> Result<(), ServeError> {
    let settings = Settings::from_env()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(ServeError::Runtime)?;
    runtime.block_on(run(settings))
}
