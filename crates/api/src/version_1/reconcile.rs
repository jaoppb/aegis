//! The reconcile request the controller sends an agent, and the status report that comes back.

use serde::{Deserialize, Serialize};

/// The path of the reconcile endpoint, served by agents.
pub const RECONCILE_PATH: &str = "/v1/reconcile";

/// The path of the status endpoint, served by the controller: agents `POST` their reports to it
/// and `GET` answers with a [`ControllerStatus`](super::ControllerStatus).
pub const STATUS_PATH: &str = "/v1/status";

/// The body of `POST /v1/reconcile`: converge the host to one commit of the repository. The
/// commit is 7 to 64 lower-case hex digits; anything else is answered `400`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconcileRequest {
    commit: String,
}

impl ReconcileRequest {
    /// A request for `commit`, written as hex digits.
    #[must_use]
    pub fn new<Text: Into<String>>(commit: Text) -> Self {
        Self {
            commit: commit.into(),
        }
    }

    /// The commit to converge to.
    #[must_use]
    pub fn commit(&self) -> &str {
        &self.commit
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

/// The body of `POST /v1/status` and of the reconcile answer: what one host did with one commit.
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
    fn a_reconcile_request_has_a_stable_wire_shape() {
        let json = serde_json::to_string(&ReconcileRequest::new("abc1234")).unwrap();
        assert_eq!(json, r#"{"commit":"abc1234"}"#);
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
