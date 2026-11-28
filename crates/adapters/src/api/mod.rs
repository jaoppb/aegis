//! The controller-agent HTTP API: the axum server that admits only tagged tailnet peers and the
//! reqwest client that calls it. The wire types live in `aegis-api`.

mod agent;
mod client;
mod enroll;
mod server;
mod wire;

pub use agent::{reconcile_routes, serve_agent, serve_agent_from_env};
pub use client::{ApiClient, ClientError};
pub use enroll::{
    CLIENT_ID_VAR, CLIENT_SECRET_VAR, CONTROLLER_REQUIRED, ControllerDesk, enroll_routes,
    enroll_routes_from_env,
};
pub use server::{ListenAddress, ServeError, serve, serve_controller, serve_from_env_with};
