//! Version 1 of the controller-agent API, served under the `/v1` path prefix.

mod health;

pub use health::{HEALTH_PATH, Health, Role, VERSION};
