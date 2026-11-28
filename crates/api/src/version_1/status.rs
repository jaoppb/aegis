//! The answer of `GET /v1/status`, served by the controller: the commit every host should be at,
//! what is wrong with the newest commit, and what each host reported.

use serde::{Deserialize, Serialize};

use super::reconcile::{StackStatus, StatusReport};

/// A commit whose tree could not be read at all, so the last good commit stays in force.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvalidCommit {
    commit: String,
    details: Vec<String>,
}

impl InvalidCommit {
    /// `commit` could not be read, for `details`.
    #[must_use]
    pub fn new<Commit: Into<String>>(commit: Commit, details: Vec<String>) -> Self {
        Self {
            commit: commit.into(),
            details,
        }
    }

    /// The unreadable commit.
    #[must_use]
    pub fn commit(&self) -> &str {
        &self.commit
    }

    /// Why its tree cannot be read.
    #[must_use]
    pub fn details(&self) -> &[String] {
        &self.details
    }
}

/// The commit the hosts should be at, and the stacks it holds that no agent will deploy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetStatus {
    commit: String,
    rejected: Vec<StackStatus>,
}

impl TargetStatus {
    /// The target `commit` with its `rejected` stacks.
    #[must_use]
    pub fn new<Commit: Into<String>>(commit: Commit, rejected: Vec<StackStatus>) -> Self {
        Self {
            commit: commit.into(),
            rejected,
        }
    }

    /// The commit.
    #[must_use]
    pub fn commit(&self) -> &str {
        &self.commit
    }

    /// The stacks that were rejected, each with its violations.
    #[must_use]
    pub fn rejected(&self) -> &[StackStatus] {
        &self.rejected
    }
}

/// Whether the controller has an agent address for a host. A host of the repository without one
/// is never told to reconcile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Agent {
    /// The controller knows where the agent listens.
    Mapped,
    /// The repository declares the host, but the controller has no address for it.
    Unmapped,
}

/// Whether a host reported the target commit. On the wire it is the string `"confirmed"` or
/// `"pending"`, or `{"failing": "<reason>"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confirmation {
    /// The host reported the target commit.
    Confirmed,
    /// The host has not reported the target commit yet.
    Pending,
    /// The last call to the host's agent failed, for this reason, and is retried every poll.
    Failing(String),
}

/// One host: whether the controller can reach an agent for it, whether it confirmed the target and
/// what it last reported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostStatus {
    host: String,
    agent: Agent,
    confirmation: Confirmation,
    report: Option<StatusReport>,
}

impl HostStatus {
    /// The status of `host`, with no report yet.
    #[must_use]
    pub fn new<Host: Into<String>>(host: Host, agent: Agent, confirmation: Confirmation) -> Self {
        Self {
            host: host.into(),
            agent,
            confirmation,
            report: None,
        }
    }

    /// The same status with the last `report` of the host, for any commit.
    #[must_use]
    pub fn with_report(self, report: StatusReport) -> Self {
        Self {
            report: Some(report),
            ..self
        }
    }

    /// The host.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Whether the controller has an agent address for the host.
    #[must_use]
    pub const fn agent(&self) -> Agent {
        self.agent
    }

    /// Whether the host reported the target commit.
    #[must_use]
    pub const fn confirmation(&self) -> &Confirmation {
        &self.confirmation
    }

    /// The last report of the host, for any commit.
    #[must_use]
    pub const fn report(&self) -> Option<&StatusReport> {
        self.report.as_ref()
    }
}

/// The body of `GET /v1/status`.
///
/// Absent values are `null`, never left out. `target` is the last good commit and stays in force
/// while `invalid` names a newer commit whose tree cannot be read, or `fetch_error` says the remote
/// cannot be reached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerStatus {
    target: Option<TargetStatus>,
    invalid: Option<InvalidCommit>,
    fetch_error: Option<String>,
    hosts: Vec<HostStatus>,
}

impl ControllerStatus {
    /// The status with the given `target` and `hosts`, and nothing wrong.
    #[must_use]
    pub const fn new(target: Option<TargetStatus>, hosts: Vec<HostStatus>) -> Self {
        Self {
            target,
            invalid: None,
            fetch_error: None,
            hosts,
        }
    }

    /// The same status with the newest commit `invalid`.
    #[must_use]
    pub fn with_invalid(self, invalid: InvalidCommit) -> Self {
        Self {
            invalid: Some(invalid),
            ..self
        }
    }

    /// The same status where the last fetch failed with `reason`.
    #[must_use]
    pub fn with_fetch_error<Reason: Into<String>>(self, reason: Reason) -> Self {
        Self {
            fetch_error: Some(reason.into()),
            ..self
        }
    }

    /// The commit the hosts should be at, once one has loaded.
    #[must_use]
    pub const fn target(&self) -> Option<&TargetStatus> {
        self.target.as_ref()
    }

    /// The newest commit, when its tree cannot be read.
    #[must_use]
    pub const fn invalid(&self) -> Option<&InvalidCommit> {
        self.invalid.as_ref()
    }

    /// Why the remote could not be fetched, when the last fetch failed.
    #[must_use]
    pub fn fetch_error(&self) -> Option<&str> {
        self.fetch_error.as_deref()
    }

    /// Every host of the repository and every host with an agent.
    #[must_use]
    pub fn hosts(&self) -> &[HostStatus] {
        &self.hosts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_controller_status_has_a_stable_wire_shape() {
        let host = HostStatus::new("jao-server", Agent::Mapped, Confirmation::Pending);
        let status = ControllerStatus::new(None, vec![host]).with_fetch_error("offline");
        let json = serde_json::to_string(&status).unwrap();
        let expected = concat!(
            r#"{"target":null,"invalid":null,"fetch_error":"offline","#,
            r#""hosts":[{"host":"jao-server","agent":"mapped","confirmation":"pending","report":null}]}"#
        );
        assert_eq!(json, expected);
    }

    #[test]
    fn the_controller_status_round_trips() {
        let target = TargetStatus::new("abcdef0", Vec::new());
        let invalid = InvalidCommit::new("1234567", vec!["hosts: unreadable".to_owned()]);
        let status = ControllerStatus::new(Some(target), Vec::new()).with_invalid(invalid);
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(
            serde_json::from_str::<ControllerStatus>(&json).unwrap(),
            status
        );
    }
}
