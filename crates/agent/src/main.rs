//! The aegis-agent binary.

use aegis_adapters::api::{ServeError, serve_from_env};
use aegis_api::version_1::Role;

fn main() -> Result<(), ServeError> {
    serve_from_env(Role::Agent)
}
