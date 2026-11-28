//! The health endpoint both servers expose.

use serde::{Deserialize, Serialize};

/// The version this module describes.
pub const VERSION: &str = "v1";

/// The path of the health endpoint.
pub const HEALTH_PATH: &str = "/v1/health";

/// Which side of the API a server is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// The controller.
    Controller,
    /// A per-host agent.
    Agent,
}

/// The body of `GET /v1/health`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    version: String,
    role: Role,
}

impl Health {
    /// The API version the server speaks.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Which side answered.
    #[must_use]
    pub const fn role(&self) -> Role {
        self.role
    }

    /// The health answer of a server with `role`.
    #[must_use]
    pub fn of(role: Role) -> Self {
        Self {
            version: VERSION.to_owned(),
            role,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_has_a_stable_wire_shape() {
        let json = serde_json::to_string(&Health::of(Role::Agent)).unwrap();
        assert_eq!(json, r#"{"version":"v1","role":"agent"}"#);
    }

    #[test]
    fn health_round_trips() {
        let json = r#"{"version":"v1","role":"controller"}"#;
        let health: Health = serde_json::from_str(json).unwrap();
        assert_eq!(health, Health::of(Role::Controller));
    }
}
