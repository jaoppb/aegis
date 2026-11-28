//! The answer of `GET /v1/fleet`, served by the controller: the commit every host should be at,
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

/// Why the controller refused a request an agent made for a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusalReason {
    /// The agent asked for more keys than the limit allows in the window.
    RateLimited,
    /// The desired state does not place the stack on the host, or the stack does not expose a
    /// service with that hostname.
    NotPlaced,
    /// The tags asked for are not exactly the tags of the stack.
    WrongTags,
    /// The agent asked to delete a node the desired state does not allow to delete: no purged
    /// stack of the host exposes the name, or another stack or a rejected stack may still use it.
    NotPurged,
}

/// A kind of request the controller refused, and how often: the same host, stack and reason are
/// one entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refused {
    host: String,
    stack: Option<String>,
    reason: RefusalReason,
    times: Counted,
}

/// How many times a request was refused, and how long ago the last time was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counted {
    count: u32,
    seconds_ago: u64,
}

impl Counted {
    /// Refused `count` times, the last one `seconds_ago` seconds ago.
    #[must_use]
    pub const fn new(count: u32, seconds_ago: u64) -> Self {
        Self { count, seconds_ago }
    }

    /// How many times.
    #[must_use]
    pub const fn count(self) -> u32 {
        self.count
    }

    /// Seconds since the last time.
    #[must_use]
    pub const fn seconds_ago(self) -> u64 {
        self.seconds_ago
    }
}

impl Refused {
    /// `host`'s requests were refused for `reason`, `times` over.
    #[must_use]
    pub fn new<Host: Into<String>>(host: Host, reason: RefusalReason, times: Counted) -> Self {
        Self {
            host: host.into(),
            stack: None,
            reason,
            times,
        }
    }

    /// The same refusal, of requests that named the stack `stack`.
    #[must_use]
    pub fn for_stack<Name: Into<String>>(self, stack: Name) -> Self {
        Self {
            stack: Some(stack.into()),
            ..self
        }
    }

    /// The host whose agent asked.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The stack the request named, when it named a valid one.
    #[must_use]
    pub fn stack(&self) -> Option<&str> {
        self.stack.as_deref()
    }

    /// Why it was refused.
    #[must_use]
    pub const fn reason(&self) -> RefusalReason {
        self.reason
    }

    /// How often, and when last.
    #[must_use]
    pub const fn times(&self) -> Counted {
        self.times
    }
}

/// What is wrong, beside what each host reported: shown flat in [`ControllerStatus`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Trouble {
    invalid: Option<InvalidCommit>,
    fetch_error: Option<String>,
    #[serde(default)]
    refusals: Vec<Refused>,
}

/// The body of `GET /v1/fleet`.
///
/// Absent values are `null`, never left out. `target` is the last good commit and stays in force
/// while `invalid` names a newer commit whose tree cannot be read, or `fetch_error` says the remote
/// cannot be reached. `refusals` lists the requests for nodes the controller refused (at most 50
/// entries, kept in memory and forgotten when the controller restarts); a status without it
/// (from an older controller) reads as having none.
///
/// An operator gets the whole fleet. An agent gets only its own host: the target with the
/// rejected stacks of its host, its own `HostStatus` and its own refusals, and never `invalid` or
/// `fetch_error`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerStatus {
    target: Option<TargetStatus>,
    #[serde(flatten)]
    trouble: Trouble,
    hosts: Vec<HostStatus>,
}

impl ControllerStatus {
    /// The status with the given `target` and `hosts`, and nothing wrong.
    #[must_use]
    pub fn new(target: Option<TargetStatus>, hosts: Vec<HostStatus>) -> Self {
        Self {
            target,
            trouble: Trouble::default(),
            hosts,
        }
    }

    /// The same status with the newest commit `invalid`.
    #[must_use]
    pub fn with_invalid(self, invalid: InvalidCommit) -> Self {
        let trouble = Trouble {
            invalid: Some(invalid),
            ..self.trouble
        };
        Self { trouble, ..self }
    }

    /// The same status where the last fetch failed with `reason`.
    #[must_use]
    pub fn with_fetch_error<Reason: Into<String>>(self, reason: Reason) -> Self {
        let trouble = Trouble {
            fetch_error: Some(reason.into()),
            ..self.trouble
        };
        Self { trouble, ..self }
    }

    /// The same status with the requests the controller `refusals` refused.
    #[must_use]
    pub fn with_refusals(self, refusals: Vec<Refused>) -> Self {
        let trouble = Trouble {
            refusals,
            ..self.trouble
        };
        Self { trouble, ..self }
    }

    /// The commit the hosts should be at, once one has loaded.
    #[must_use]
    pub const fn target(&self) -> Option<&TargetStatus> {
        self.target.as_ref()
    }

    /// The newest commit, when its tree cannot be read.
    #[must_use]
    pub const fn invalid(&self) -> Option<&InvalidCommit> {
        self.trouble.invalid.as_ref()
    }

    /// Why the remote could not be fetched, when the last fetch failed.
    #[must_use]
    pub fn fetch_error(&self) -> Option<&str> {
        self.trouble.fetch_error.as_deref()
    }

    /// The requests for nodes the controller refused.
    #[must_use]
    pub fn refusals(&self) -> &[Refused] {
        &self.trouble.refusals
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
            r#"{"target":null,"invalid":null,"fetch_error":"offline","refusals":[],"#,
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

    #[test]
    fn a_status_without_refusals_reads_as_having_none() {
        let json = r#"{"target":null,"invalid":null,"fetch_error":null,"hosts":[]}"#;
        let status: ControllerStatus = serde_json::from_str(json).unwrap();
        assert!(status.refusals().is_empty());
    }

    #[test]
    fn a_refusal_has_a_stable_wire_shape() {
        let times = Counted::new(2, 30);
        let refused =
            Refused::new("jao-server", RefusalReason::RateLimited, times).for_stack("vault");
        let json = serde_json::to_string(&refused).unwrap();
        let expected = concat!(
            r#"{"host":"jao-server","stack":"vault","reason":"rate_limited","#,
            r#""times":{"count":2,"seconds_ago":30}}"#
        );
        assert_eq!(json, expected);
    }
}
