//! The sync resource: an operator asks the controller to make hosts converge now, instead of at
//! the next poll. `POST /v1/syncs` syncs every host, `POST /v1/hosts/{host}/syncs` one host.

use serde::{Deserialize, Serialize};

use super::reconcile::StatusReport;

/// The route that syncs every host of the controller.
///
/// `POST` fetches the repository, tells each agent to converge to the newest commit and answers
/// a [`SyncResponse`] with one entry per host once they have all answered. Statuses: `200` the
/// syncs ran (a host that failed is listed with its reason), `400` the stack name is malformed,
/// `403` the caller is not an operator, `409` no commit has loaded yet or the newest commit cannot
/// be read, `502` the repository cannot be fetched, `503` the whois lookup failed.
pub const SYNCS_PATH: &str = "/v1/syncs";

/// The route that syncs one host, answered like [`SYNCS_PATH`] with that host alone. `404` means
/// the controller has no agent for the host and `400` that the host name is malformed.
pub const HOST_SYNCS_PATH: &str = "/v1/hosts/{host}/syncs";

/// The path that syncs `host`, a directory name under `hosts/`.
#[must_use]
pub fn host_syncs_path(host: &str) -> String {
    format!("/v1/hosts/{host}/syncs")
}

/// The body of a sync request: which stack to sync, when not all of them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyncRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stack: Option<String>,
}

impl SyncRequest {
    /// A request that syncs every stack.
    #[must_use]
    pub const fn everything() -> Self {
        Self { stack: None }
    }

    /// The same request, limited to the stack named `stack`.
    #[must_use]
    pub fn only<Text: Into<String>>(stack: Text) -> Self {
        Self {
            stack: Some(stack.into()),
        }
    }

    /// The stack to sync, when only one.
    #[must_use]
    pub fn stack(&self) -> Option<&str> {
        self.stack.as_deref()
    }
}

/// What a sync did on one host: the report of its agent, or why there is none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostSync {
    host: String,
    report: Option<StatusReport>,
    failure: Option<String>,
}

impl HostSync {
    /// `host` converged, and its agent reported `report`.
    #[must_use]
    pub fn done<Host: Into<String>>(host: Host, report: StatusReport) -> Self {
        Self {
            host: host.into(),
            report: Some(report),
            failure: None,
        }
    }

    /// `host` could not be synced, for `reason`.
    #[must_use]
    pub fn failed<Host: Into<String>, Reason: Into<String>>(host: Host, reason: Reason) -> Self {
        Self {
            host: host.into(),
            report: None,
            failure: Some(reason.into()),
        }
    }

    /// The host.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// What its agent reported, when the call worked.
    #[must_use]
    pub const fn report(&self) -> Option<&StatusReport> {
        self.report.as_ref()
    }

    /// Why the host was not synced, when the call failed.
    #[must_use]
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }
}

/// The answer of a sync: one entry per host that was asked.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncResponse {
    hosts: Vec<HostSync>,
}

impl SyncResponse {
    /// The answer for `hosts`.
    #[must_use]
    pub const fn new(hosts: Vec<HostSync>) -> Self {
        Self { hosts }
    }

    /// The hosts, in name order.
    #[must_use]
    pub fn hosts(&self) -> &[HostSync] {
        &self.hosts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_for_everything_is_an_empty_object() {
        let json = serde_json::to_string(&SyncRequest::everything()).unwrap();
        assert_eq!(json, "{}");
    }

    #[test]
    fn a_request_for_a_stack_names_it() {
        let json = serde_json::to_string(&SyncRequest::only("web")).unwrap();
        assert_eq!(json, r#"{"stack":"web"}"#);
        let back: SyncRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.stack(), Some("web"));
    }

    #[test]
    fn an_empty_body_object_syncs_everything() {
        let request: SyncRequest = serde_json::from_str("{}").unwrap();
        assert_eq!(request.stack(), None);
    }

    #[test]
    fn an_unknown_field_is_refused() {
        assert!(serde_json::from_str::<SyncRequest>(r#"{"stacks":"web"}"#).is_err());
    }

    #[test]
    fn a_host_path_names_the_host_under_hosts() {
        assert_eq!(host_syncs_path("jao-server"), "/v1/hosts/jao-server/syncs");
    }

    #[test]
    fn a_failed_host_has_a_reason_and_no_report() {
        let failed = HostSync::failed("h", "the agent is unreachable");
        assert!(failed.report().is_none());
        assert_eq!(failed.failure(), Some("the agent is unreachable"));
    }
}
