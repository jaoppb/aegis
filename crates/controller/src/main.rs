//! The aegis-controller binary.

use aegis_adapters::api::{ServeError, enroll_routes_from_env, serve_from_env_with};
use aegis_api::version_1::Role;

fn main() -> Result<(), ServeError> {
    serve_from_env_with(Role::Controller, enroll_routes_from_env()?)
}
