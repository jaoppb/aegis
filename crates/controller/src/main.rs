//! The aegis-controller binary.

use std::env;

use aegis_adapters::api::{ListenAddress, ServeError, serve_blocking};
use aegis_api::version_1::Role;

/// The environment variable that names the address to listen on, normally a tailnet address.
const LISTEN_VAR: &str = "AEGIS_LISTEN";

/// Why the controller did not run.
#[derive(Debug, thiserror::Error)]
enum StartError {
    /// `AEGIS_LISTEN` is not set.
    #[error("{LISTEN_VAR} is not set")]
    NotConfigured,
    /// `AEGIS_LISTEN` is not a socket address such as `100.64.0.7:7480`.
    #[error("{LISTEN_VAR} is not a socket address: {0}")]
    BadAddress(#[from] std::net::AddrParseError),
    /// The server did not start or failed.
    #[error(transparent)]
    Serve(#[from] ServeError),
}

fn main() -> Result<(), StartError> {
    let text = env::var(LISTEN_VAR).map_err(|_| StartError::NotConfigured)?;
    let listen: ListenAddress = text.parse()?;
    Ok(serve_blocking(Role::Controller, listen)?)
}
