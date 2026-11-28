//! The controller-agent HTTP API: the axum server that admits only tagged tailnet peers and the
//! reqwest client that calls it. The wire types live in `aegis-api`.

mod agent;
mod client;
mod controller;
mod controller_env;
mod enroll;
mod nodes;
mod server;
mod wire;

pub use agent::{reconcile_routes, serve_agent, serve_agent_from_env};
pub use client::{ApiClient, ClientError};
pub use controller::{AgentAddress, Hub};
pub use controller_env::{REPOSITORY_REQUIRED, serve_controller_from_env};
pub use enroll::{
    CLIENT_ID_VAR, CLIENT_SECRET_VAR, CONTROLLER_REQUIRED, ControllerDesk, enroll_routes,
};
pub use nodes::node_routes;
pub use server::{
    ListenAddress, Operated, Routes, ServeError, serve, serve_controller, serve_controller_for,
};
