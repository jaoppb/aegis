//! The controller-agent HTTP API: the axum server that admits only tagged tailnet peers and the
//! reqwest client that calls it. The wire types live in `aegis-api`.

mod client;
mod server;

pub use client::{ApiClient, ClientError};
pub use server::{ListenAddress, ServeError, serve, serve_from_env};
