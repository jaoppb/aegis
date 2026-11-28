//! The enrolment request an agent sends the controller for a sidecar's tailnet node, and the
//! answer: the tailnet's name and, for a node that has never joined, a freshly minted auth key.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The path of the enrolment endpoint, served by the controller.
pub const ENROLL_PATH: &str = "/v1/enroll";

/// What the agent found in the sidecar's state volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidecarVolume {
    /// Nothing is stored: the node has never joined the tailnet.
    Empty,
    /// A node identity is stored, so a restart reuses it.
    Populated,
}

/// The body of `POST /v1/enroll`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollRequest {
    hostname: String,
    tags: Vec<String>,
    state: SidecarVolume,
}

impl EnrollRequest {
    /// A request for the node `hostname`, tagged `tags` such as `tag:app`, whose state volume is
    /// `state`.
    #[must_use]
    pub fn new<Host: Into<String>>(
        hostname: Host,
        tags: Vec<String>,
        state: SidecarVolume,
    ) -> Self {
        Self {
            hostname: hostname.into(),
            tags,
            state,
        }
    }

    /// The hostname of the node, a DNS label.
    #[must_use]
    pub fn hostname(&self) -> &str {
        &self.hostname
    }

    /// The ACL tags the node asks for.
    #[must_use]
    pub fn tags(&self) -> &[String] {
        &self.tags
    }

    /// The state of the sidecar's volume.
    #[must_use]
    pub const fn state(&self) -> SidecarVolume {
        self.state
    }
}

/// The answer of `POST /v1/enroll`. The key is a secret: `Debug` hides it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollResponse {
    tailnet: String,
    key: Option<String>,
}

impl EnrollResponse {
    /// The answer for a node of `tailnet`, with `key` when one was minted.
    #[must_use]
    pub fn new<Tailnet: Into<String>>(tailnet: Tailnet, key: Option<String>) -> Self {
        Self {
            tailnet: tailnet.into(),
            key,
        }
    }

    /// The `MagicDNS` suffix of the tailnet, such as `taildfab0.ts.net`.
    #[must_use]
    pub fn tailnet(&self) -> &str {
        &self.tailnet
    }

    /// The single-use auth key, present only when the node had no state.
    #[must_use]
    pub fn key(&self) -> Option<&str> {
        self.key.as_deref()
    }
}

impl fmt::Debug for EnrollResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let key = self.key.as_ref().map_or("none", |_| "<redacted>");
        write!(formatter, "EnrollResponse({}, key: {key})", self.tailnet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_enroll_request_has_a_stable_wire_shape() {
        let request = EnrollRequest::new(
            "vaultwarden",
            vec!["tag:app".to_owned()],
            SidecarVolume::Empty,
        );
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(
            json,
            r#"{"hostname":"vaultwarden","tags":["tag:app"],"state":"empty"}"#
        );
    }

    #[test]
    fn an_enroll_request_with_an_unknown_field_is_refused() {
        let json = r#"{"hostname":"a","tags":[],"state":"populated","extra":1}"#;
        assert!(serde_json::from_str::<EnrollRequest>(json).is_err());
    }

    #[test]
    fn an_enroll_response_has_a_stable_wire_shape() {
        let json = r#"{"tailnet":"taildfab0.ts.net","key":null}"#;
        let response: EnrollResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response, EnrollResponse::new("taildfab0.ts.net", None));
        assert_eq!(serde_json::to_string(&response).unwrap(), json);
    }

    #[test]
    fn an_enroll_response_never_prints_its_key() {
        let response = EnrollResponse::new("t.ts.net", Some("tskey-secret".to_owned()));
        assert!(!format!("{response:?}").contains("tskey-secret"));
    }
}
