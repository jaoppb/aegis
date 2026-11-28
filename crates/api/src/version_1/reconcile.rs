//! The reconcile request the controller sends an agent, and the status report that comes back.

use serde::{Deserialize, Serialize};

/// The path of the reconcile endpoint, served by agents.
pub const RECONCILIATIONS_PATH: &str = "/v1/reconciliations";

/// The path of the fleet resource, served by the controller: `GET` answers with a
/// [`ControllerStatus`](super::ControllerStatus), the whole fleet for an operator and the caller's
/// own host for an agent.
pub const FLEET_PATH: &str = "/v1/fleet";

/// The path of the status of one host, served by the controller.
///
/// The agent of `{host}` `PUT`s its [`StatusReport`] there. Statuses: `204` the report was recorded, `400` the commit or the host
/// of the body is malformed or is not the host of the path, `403` the caller is no agent of the
/// host in the path, `404` the controller has no agent for the host.
pub const HOST_STATUS_PATH: &str = "/v1/hosts/{host}/status";

/// The path of the status of `host`, a directory name under `hosts/`.
#[must_use]
pub fn host_status_path(host: &str) -> String {
    format!("/v1/hosts/{host}/status")
}

/// The most bytes the controller takes from an agent in one message: 1 MiB.
///
/// It applies to the answer of `POST /v1/reconciliations` and to the body of an agent's
/// `PUT /v1/hosts/{host}/status`. A larger answer is abandoned without being kept, and shows in the status of
/// the host as a failure.
pub const MAX_REPLY_BYTES: usize = 1_048_576;

/// The body of `POST /v1/reconciliations`: converge the host to one commit of the repository. The
/// commit is 7 to 64 lower-case hex digits; anything else is answered `400`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconcileRequest {
    commit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stack: Option<String>,
}

impl ReconcileRequest {
    /// A request for `commit`, written as hex digits.
    #[must_use]
    pub fn new<Text: Into<String>>(commit: Text) -> Self {
        Self {
            commit: commit.into(),
            stack: None,
        }
    }

    /// The same request, limited to the stack named `stack`: the agent leaves the others alone
    /// and does not report to the controller, because a partial report must not stand for the
    /// host.
    #[must_use]
    pub fn only<Text: Into<String>>(self, stack: Text) -> Self {
        Self {
            stack: Some(stack.into()),
            ..self
        }
    }

    /// The commit to converge to.
    #[must_use]
    pub fn commit(&self) -> &str {
        &self.commit
    }

    /// The only stack to converge, when not all of them.
    #[must_use]
    pub fn stack(&self) -> Option<&str> {
        self.stack.as_deref()
    }
}

/// What happened to a stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StackState {
    /// The stack was brought up.
    Running,
    /// Compose failed to bring the stack up.
    Failed,
    /// The stack broke a rule and was not deployed.
    Invalid,
    /// The stack left the repository: it was taken down and its volumes are kept. The details
    /// list what was kept; `directory` is where the stack used to live.
    Orphaned,
    /// The stack's tombstone was carried out and its directory can be deleted from the repository.
    Purged,
}

/// The result for one stack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackStatus {
    directory: String,
    state: StackState,
    details: Vec<String>,
}

impl StackStatus {
    /// The `state` of the stack in `directory`, with `details` such as the violations or the
    /// failure.
    #[must_use]
    pub fn new<Text: Into<String>>(
        directory: Text,
        state: StackState,
        details: Vec<String>,
    ) -> Self {
        Self {
            directory: directory.into(),
            state,
            details,
        }
    }

    /// The stack directory, such as `hosts/jao-server/pihole`.
    #[must_use]
    pub fn directory(&self) -> &str {
        &self.directory
    }

    /// What happened to the stack.
    #[must_use]
    pub const fn state(&self) -> StackState {
        self.state
    }

    /// The violations or the failure message; empty for a running stack.
    #[must_use]
    pub fn details(&self) -> &[String] {
        &self.details
    }
}

/// The body of `PUT /v1/hosts/{host}/status` and of the reconcile answer: what one host did with one commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusReport {
    host: String,
    commit: String,
    stacks: Vec<StackStatus>,
}

impl StatusReport {
    /// The report of `host` for `commit` with one status per stack.
    #[must_use]
    pub fn new<Host: Into<String>, Commit: Into<String>>(
        host: Host,
        commit: Commit,
        stacks: Vec<StackStatus>,
    ) -> Self {
        Self {
            host: host.into(),
            commit: commit.into(),
            stacks,
        }
    }

    /// The host that reconciled.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The commit it reconciled.
    #[must_use]
    pub fn commit(&self) -> &str {
        &self.commit
    }

    /// One status per stack.
    #[must_use]
    pub fn stacks(&self) -> &[StackStatus] {
        &self.stacks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_of_a_host_lives_under_that_host() {
        assert_eq!(
            host_status_path("jao-server"),
            "/v1/hosts/jao-server/status"
        );
    }

    #[test]
    fn a_reconcile_request_has_a_stable_wire_shape() {
        let json = serde_json::to_string(&ReconcileRequest::new("abc1234")).unwrap();
        assert_eq!(json, r#"{"commit":"abc1234"}"#);
    }

    #[test]
    fn a_request_for_one_stack_names_it_and_an_old_body_still_reads() {
        let json = serde_json::to_string(&ReconcileRequest::new("abc1234").only("web")).unwrap();
        assert_eq!(json, r#"{"commit":"abc1234","stack":"web"}"#);
        let old: ReconcileRequest = serde_json::from_str(r#"{"commit":"abc1234"}"#).unwrap();
        assert_eq!(old.stack(), None);
    }

    #[test]
    fn the_removal_states_have_a_stable_wire_shape() {
        let orphaned = serde_json::to_string(&StackState::Orphaned).unwrap();
        let purged = serde_json::to_string(&StackState::Purged).unwrap();
        assert_eq!(
            (orphaned.as_str(), purged.as_str()),
            (r#""orphaned""#, r#""purged""#)
        );
    }

    #[test]
    fn a_status_report_has_a_stable_wire_shape() {
        let stack = StackStatus::new("hosts/h/web", StackState::Failed, vec!["boom".to_owned()]);
        let report = StatusReport::new("h", "abc1234", vec![stack]);
        let json = serde_json::to_string(&report).unwrap();
        let expected = r#"{"host":"h","commit":"abc1234","stacks":[{"directory":"hosts/h/web","state":"failed","details":["boom"]}]}"#;
        assert_eq!(json, expected);
        assert_eq!(serde_json::from_str::<StatusReport>(&json).unwrap(), report);
    }
}
