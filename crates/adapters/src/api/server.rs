//! The server side of the `/v1` API.
//!
//! Every request goes through [`Admitted`], which resolves the peer's tailnet identity and checks
//! its ACL tag before any handler runs.

use std::env::VarError;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;

use aegis_api::version_1::{HEALTH_PATH, Health, Role};
use aegis_application::{AccessError, Authorizer, PeerAddress, WhoIs};
use aegis_domain::{AegisRole, ValueError};
use axum::extract::{ConnectInfo, FromRequestParts, Request};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::middleware::{Next, from_fn_with_state};
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use tokio::net::TcpListener;

use crate::whois::TailscaleWhois;

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

/// Proof that the peer of a request carries the required tag.
struct Admitted;

impl<Lookup> FromRequestParts<Gate<Lookup>> for Admitted
where
    Lookup: WhoIs + Send + Sync + 'static,
{
    type Rejection = StatusCode;

    async fn from_request_parts(
        parts: &mut Parts,
        gate: &Gate<Lookup>,
    ) -> Result<Self, Self::Rejection> {
        let peer = peer_of(parts)?;
        gate.authorize(&peer)
            .await
            .map_err(|error| status_of(&error))?;
        Ok(Self)
    }
}

/// The peer address the transport recorded. A request without one means the server was wired
/// without connection info, and it is refused.
fn peer_of(parts: &Parts) -> Result<PeerAddress, StatusCode> {
    let connection = parts.extensions.get::<ConnectInfo<SocketAddr>>();
    let ConnectInfo(address) = connection.ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(PeerAddress::new(address.to_string()))
}

const fn status_of(error: &AccessError) -> StatusCode {
    match error {
        AccessError::Denied => StatusCode::FORBIDDEN,
        AccessError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}

async fn admit(_admitted: Admitted, request: Request, next: Next) -> Response {
    next.run(request).await
}

/// The `/v1` routes of a server with `role`, and the `extra` routes it adds, all behind the tag
/// check of `authorizer`.
fn router<Lookup>(role: Role, authorizer: Authorizer<Lookup>, extra: Router) -> Router
where
    Lookup: WhoIs + Send + Sync + 'static,
{
    let gate = Arc::new(authorizer);
    Router::new()
        .route(
            HEALTH_PATH,
            get(move || async move { Json(Health::of(role)) }),
        )
        .merge(extra)
        .layer(from_fn_with_state(gate, admit))
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
    serve_routes(listener, Guard::new(role, whois), Router::new()).await
}

/// Who a server is and how it resolves its peers.
pub(super) struct Guard<Lookup> {
    role: Role,
    whois: Lookup,
}

impl<Lookup> Guard<Lookup> {
    /// A guard for a server with `role` that resolves peers with `whois`.
    pub(super) const fn new(role: Role, whois: Lookup) -> Self {
        Self { role, whois }
    }
}

/// Like [`serve`], with the `extra` routes added behind the same tag check.
///
/// # Errors
///
/// The same as [`serve`].
pub(super) async fn serve_routes<Lookup>(
    listener: TcpListener,
    guard: Guard<Lookup>,
    extra: Router,
) -> Result<(), ServeError>
where
    Lookup: WhoIs + Send + Sync + 'static,
{
    let authorizer = Authorizer::for_server(guard.whois, role_of(guard.role))?;
    let service =
        router(guard.role, authorizer, extra).into_make_service_with_connect_info::<SocketAddr>();
    axum::serve(listener, service)
        .await
        .map_err(ServeError::Serve)
}

async fn run(role: Role, listen: ListenAddress, extra: Router) -> Result<(), ServeError> {
    let listener = TcpListener::bind(listen.0)
        .await
        .map_err(|cause| ServeError::Bind {
            address: listen.0,
            cause,
        })?;
    serve_routes(listener, Guard::new(role, TailscaleWhois::new()), extra).await
}

pub(super) fn listen_from(variable: Result<String, VarError>) -> Result<ListenAddress, ServeError> {
    let text = variable.map_err(|_| ServeError::NotConfigured)?;
    Ok(text.parse()?)
}

/// Run a server with `role` on its own runtime until it fails, listening on the address in
/// `AEGIS_LISTEN`. It resolves peers with the `tailscale` CLI and admits only the other side's tag.
///
/// # Errors
///
/// Returns a [`ServeError`] when `AEGIS_LISTEN` is missing or invalid, the runtime cannot start,
/// the address cannot be bound or the server fails.
pub fn serve_from_env(role: Role) -> Result<(), ServeError> {
    serve_from_env_with(role, Router::new())
}

/// Like [`serve_from_env`], with the `extra` routes added behind the same tag check.
///
/// # Errors
///
/// The same as [`serve_from_env`].
pub fn serve_from_env_with(role: Role, extra: Router) -> Result<(), ServeError> {
    let listen = listen_from(std::env::var(LISTEN_VAR))?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(ServeError::Runtime)?;
    runtime.block_on(run(role, listen, extra))
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
