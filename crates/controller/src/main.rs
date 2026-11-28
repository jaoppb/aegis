//! The aegis-controller binary.

use aegis_adapters::api::{ServeError, serve_controller_from_env};

fn main() -> Result<(), ServeError> {
    serve_controller_from_env()
}
