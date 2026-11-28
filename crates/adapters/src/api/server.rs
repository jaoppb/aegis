//! The server side of the `/v1` API.
//!
//! Every request goes through [`Admitted`], which resolves the peer's tailnet identity and checks
//! its ACL tag before any handler runs.

use std::env::VarError;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;

use aegis_api::version_1::{HEALTH_PATH, Health, Role};
use aegis_application::{AccessError, Admit, Authorizer, PeerAddress, WhoIs};
use aegis_domain::{AegisRole, Logins, PeerIdentity, ValueError};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::middleware::{Next, from_fn_with_state};
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use tokio::net::TcpListener;

/// The environment variable that names the address to listen on, normally a tailnet address.
pub(super) const LISTEN_VAR: &str = "AEGIS_LISTEN";

/// A server could not start or stopped.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    /// The async runtime could not be built.
    #[error("cannot start the async runtime: {0}")]
    Runtime(std::io::Error),
    /// The listen address could not be bound.
    #[error("cannot listen on {address}: {cause}")]
    Bind {
        /// The address that was asked for.
        address: SocketAddr,
        /// Why the bind failed.
        cause: std::io::Error,
    },
    /// The server stopped on an IO error.
    #[error("the server failed: {0}")]
    Serve(std::io::Error),
    /// `AEGIS_LISTEN` is not set.
    #[error("{LISTEN_VAR} is not set")]
    NotConfigured,
    /// `AEGIS_LISTEN` is not a socket address such as `100.64.0.7:7480`.
    #[error("{LISTEN_VAR} is not a socket address: {0}")]
    BadAddress(#[from] std::net::AddrParseError),
    /// The required ACL tag is not valid.
    #[error("invalid ACL tag: {0}")]
    Tag(#[from] ValueError),
    /// Decrypted files left by an earlier run could not be removed.
    #[error("cannot remove stale decrypted files: {0}")]
    Sweep(aegis_application::StoreError),
    /// A required environment variable is not set.
    #[error("{0} is not set")]
    Missing(&'static str),
    /// An environment variable holds an invalid value.
    #[error("{variable} is invalid: {cause}")]
    Invalid {
        /// The variable.
        variable: &'static str,
        /// What is wrong with it.
        cause: String,
    },
    /// The client for the controller could not be built.
    #[error("cannot build the controller client: {0}")]
    Client(#[from] super::ClientError),
}

/// The address a server listens on, normally an address of the tailnet interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListenAddress(SocketAddr);

impl ListenAddress {
    pub(super) const fn socket(self) -> SocketAddr {
        self.0
    }
}

impl FromStr for ListenAddress {
    type Err = std::net::AddrParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        text.parse().map(Self)
    }
}

const fn role_of(role: Role) -> AegisRole {
    match role {
        Role::Agent => AegisRole::Agent,
        Role::Controller => AegisRole::Controller,
    }
}

/// The shared state of the guard: the authorizer every request goes through.
type Gate<Lookup> = Arc<Authorizer<Lookup>>;

/// The peer address the transport recorded. A request without one means the server was wired
/// without connection info, and it is refused.
fn peer_of(request: &Request) -> Result<PeerAddress, StatusCode> {
    let connection = request.extensions().get::<ConnectInfo<SocketAddr>>();
    let ConnectInfo(address) = connection.ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(PeerAddress::new(address.to_string()))
}

const fn status_of(error: &AccessError) -> StatusCode {
    match error {
        AccessError::Denied => StatusCode::FORBIDDEN,
        AccessError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}

/// Resolve the peer of a request before any route runs, and keep its identity for the route's own
/// check. A peer that is no tailnet node, or that cannot be resolved, never reaches a route.
async fn identify<Lookup>(
    State(gate): State<Gate<Lookup>>,
    mut request: Request,
    next: Next,
) -> Result<Response, StatusCode>
where
    Lookup: WhoIs + Send + Sync + 'static,
{
    let peer = peer_of(&request)?;
    let identity = gate
        .identify(&peer)
        .await
        .map_err(|error| status_of(&error))?;
    request.extensions_mut().insert(identity);
    Ok(next.run(request).await)
}

/// The authorizer and the callers one group of routes admits.
struct Door<Lookup> {
    gate: Gate<Lookup>,
    admit: Admit,
}

impl<Lookup> Clone for Door<Lookup> {
    fn clone(&self) -> Self {
        Self {
            gate: Arc::clone(&self.gate),
            admit: self.admit,
        }
    }
}

/// Let the request through when the identity [`identify`] resolved is one the route admits.
async fn admit<Lookup>(
    State(door): State<Door<Lookup>>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode>
where
    Lookup: WhoIs + Send + Sync + 'static,
{
    let identity = request.extensions().get::<PeerIdentity>();
    let known = identity.ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    door.gate
        .admits(known, door.admit)
        .map_err(|error| status_of(&error))?;
    Ok(next.run(request).await)
}

/// The routes of a server, by who may call them. A route is open to the tagged peer unless it is
/// added on purpose for operators, so a new route fails closed.
#[derive(Debug, Default)]
pub struct Routes {
    tagged: Router,
    operators: Router,
    either: Router,
}

impl Routes {
    /// The routes only the tagged peer may call.
    #[must_use]
    pub fn tagged(self, routes: Router) -> Self {
        Self {
            tagged: self.tagged.merge(routes),
            ..self
        }
    }

    /// The routes only an operator may call.
    #[must_use]
    pub fn operators(self, routes: Router) -> Self {
        Self {
            operators: self.operators.merge(routes),
            ..self
        }
    }

    /// The routes both the tagged peer and an operator may call.
    #[must_use]
    pub fn either(self, routes: Router) -> Self {
        Self {
            either: self.either.merge(routes),
            ..self
        }
    }
}

impl From<Router> for Routes {
    fn from(routes: Router) -> Self {
        Self::default().tagged(routes)
    }
}

fn guarded<Lookup>(routes: Router, gate: &Gate<Lookup>, admit_who: Admit) -> Router
where
    Lookup: WhoIs + Send + Sync + 'static,
{
    let door = Door {
        gate: Arc::clone(gate),
        admit: admit_who,
    };
    routes.layer(from_fn_with_state(door, admit::<Lookup>))
}

/// The `/v1` routes of a server with `role`, and the `routes` it adds, each group behind its own
/// admission check and all behind the resolution of the peer.
fn router<Lookup>(role: Role, authorizer: Authorizer<Lookup>, routes: Routes) -> Router
where
    Lookup: WhoIs + Send + Sync + 'static,
{
    let gate = Arc::new(authorizer);
    let health = Router::new().route(
        HEALTH_PATH,
        get(move || async move { Json(Health::of(role)) }),
    );
    let guarded_routes = [
        guarded(routes.tagged, &gate, Admit::Tagged),
        guarded(routes.operators, &gate, Admit::Operator),
        guarded(routes.either.merge(health), &gate, Admit::Either),
    ];
    let merged = guarded_routes
        .into_iter()
        .fold(Router::new(), |all, group| all.merge(group));
    merged.layer(from_fn_with_state(gate, identify::<Lookup>))
}

/// Serve the API of a server with `role` on `listener` until it fails. Peers are resolved with
/// `whois`, and only the other side's tag is admitted.
///
/// # Errors
///
/// Returns [`ServeError::Serve`] when the server stops on an IO error, and [`ServeError::Tag`] if
/// the built-in ACL tag were invalid.
pub async fn serve<Lookup>(
    listener: TcpListener,
    role: Role,
    whois: Lookup,
) -> Result<(), ServeError>
where
    Lookup: WhoIs + Send + Sync + 'static,
{
    serve_routes(listener, Guard::new(role, whois), Routes::default()).await
}

/// Serve a controller on `listener`: the health endpoint and the extra `routes`, behind the tag
/// check that admits only agents.
///
/// # Errors
///
/// The same as [`serve`].
pub async fn serve_controller<Lookup>(
    listener: TcpListener,
    whois: Lookup,
    routes: Router,
) -> Result<(), ServeError>
where
    Lookup: WhoIs + Send + Sync + 'static,
{
    let guard = Guard::new(Role::Controller, whois);
    serve_routes(listener, guard, Routes::from(routes)).await
}

/// A controller's way of knowing its callers: how it resolves a peer, and which people it admits
/// as operators on the routes opened to them.
#[derive(Debug)]
pub struct Operated<Lookup> {
    whois: Lookup,
    operators: Logins,
}

impl<Lookup> Operated<Lookup> {
    /// Resolve peers with `whois` and admit the people `operators`. With no operators, none is
    /// admitted.
    #[must_use]
    pub const fn new(whois: Lookup, operators: Logins) -> Self {
        Self { whois, operators }
    }
}

/// Serve a controller on `listener` that also admits operators on the routes `routes` opens to
/// them. Whatever is not opened to operators stays for agents alone.
///
/// # Errors
///
/// The same as [`serve`].
pub async fn serve_controller_for<Lookup>(
    listener: TcpListener,
    callers: Operated<Lookup>,
    routes: Routes,
) -> Result<(), ServeError>
where
    Lookup: WhoIs + Send + Sync + 'static,
{
    let guard = Guard::new(Role::Controller, callers.whois).with_operators(callers.operators);
    serve_routes(listener, guard, routes).await
}

/// Who a server is and how it resolves its peers.
pub(super) struct Guard<Lookup> {
    role: Role,
    whois: Lookup,
    operators: Logins,
}

impl<Lookup> Guard<Lookup> {
    /// A guard for a server with `role` that resolves peers with `whois` and admits no operator.
    pub(super) fn new(role: Role, whois: Lookup) -> Self {
        Self {
            role,
            whois,
            operators: Logins::default(),
        }
    }

    /// The same guard, which also admits `operators` on the routes that allow them.
    fn with_operators(self, operators: Logins) -> Self {
        Self { operators, ..self }
    }
}

/// Like [`serve`], with the `routes` added, each group behind its own admission check.
///
/// # Errors
///
/// The same as [`serve`].
pub(super) async fn serve_routes<Lookup>(
    listener: TcpListener,
    guard: Guard<Lookup>,
    routes: Routes,
) -> Result<(), ServeError>
where
    Lookup: WhoIs + Send + Sync + 'static,
{
    let authorizer = Authorizer::for_server(guard.whois, role_of(guard.role))?;
    let authorizer = authorizer.with_operators(guard.operators);
    let service =
        router(guard.role, authorizer, routes).into_make_service_with_connect_info::<SocketAddr>();
    axum::serve(listener, service)
        .await
        .map_err(ServeError::Serve)
}

pub(super) fn listen_from(variable: Result<String, VarError>) -> Result<ListenAddress, ServeError> {
    let text = variable.map_err(|_| ServeError::NotConfigured)?;
    Ok(text.parse()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_takes_the_domain_role_of_its_side() {
        assert_eq!(role_of(Role::Agent), AegisRole::Agent);
        assert_eq!(role_of(Role::Controller), AegisRole::Controller);
    }

    #[test]
    fn a_missing_variable_is_not_configured() {
        let result = listen_from(Err(VarError::NotPresent));
        assert!(matches!(result, Err(ServeError::NotConfigured)));
    }

    #[test]
    fn a_bad_variable_is_a_bad_address() {
        let result = listen_from(Ok("nowhere".to_owned()));
        assert!(matches!(result, Err(ServeError::BadAddress(_))));
    }

    #[test]
    fn a_good_variable_is_a_listen_address() {
        let result = listen_from(Ok("100.64.0.7:7480".to_owned()));
        assert_eq!(result.unwrap(), "100.64.0.7:7480".parse().unwrap());
    }

    #[test]
    fn listen_addresses_parse_socket_addresses() {
        assert!("100.64.0.7:7480".parse::<ListenAddress>().is_ok());
        assert!("not-an-address".parse::<ListenAddress>().is_err());
    }
}
