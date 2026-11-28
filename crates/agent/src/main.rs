//! The aegis-agent binary.

use aegis_adapters::api::{ServeError, serve_agent_from_env};

fn main() -> Result<(), ServeError> {
    serve_agent_from_env()
}
