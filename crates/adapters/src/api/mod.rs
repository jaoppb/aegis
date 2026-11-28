//! The controller-agent HTTP API: the axum server that admits only tagged tailnet peers and the
//! reqwest client that calls it. The wire types live in `aegis-api`.

mod agent;
mod client;
mod server;
mod wire;

pub use agent::{reconcile_routes, serve_agent, serve_agent_from_env};
pub use client::{ApiClient, BaseUrl, ClientError};
pub use server::{ListenAddress, ServeError, serve, serve_from_env};
