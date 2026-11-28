//! What deploying a stack needs and what it leaves behind: the project name, the commit, and the
//! per-stack result of a reconcile.

use std::fmt;

use crate::value::{HostName, RepoPath, StackName, ValueError, collection};
use crate::violation::Violations;

/// The longest text a [`Detail`] keeps, so a runaway tool output cannot bloat a report.
const MAX_DETAIL_CHARS: usize = 2000;

/// The prefix of every Compose project Aegis owns.
const PROJECT_PREFIX: &str = "aegis-";

const fn is_project_char(candidate: char) -> bool {
    candidate.is_ascii_lowercase() || candidate.is_ascii_digit() || matches!(candidate, '-' | '_')
}

const fn is_hex(candidate: char) -> bool {
    candidate.is_ascii_digit() || matches!(candidate, 'a'..='f')
}

/// The Compose project of a stack: `aegis-<stack>`, so Aegis-owned resources are identifiable.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProjectName(String);

impl ProjectName {
    /// The project of `stack`.
    ///
    /// # Errors
    ///
    /// Returns [`ValueError::ForbiddenChar`] when the stack name holds a character Compose does
    /// not accept in a project name (an upper-case letter or a dot).
    pub fn of(stack: &StackName) -> Result<Self, ValueError> {
        let bad = stack
            .as_str()
            .chars()
            .find(|found| !is_project_char(*found));
        bad.map_or_else(
            || Ok(Self(format!("{PROJECT_PREFIX}{stack}"))),
            |found| Err(ValueError::ForbiddenChar(found)),
        )
    }

    /// The project name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProjectName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A git commit id in lower-case hex, abbreviated or full. Because it is only hex it can never be
/// read as a command-line option or a path.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommitSha(String);

impl CommitSha {
    /// Validate `value` as 7 to 64 lower-case hex digits.
    ///
    /// # Errors
    ///
    /// Returns [`ValueError::Empty`] for an empty value and [`ValueError::ForbiddenChar`] for a
    /// character that is not a lower-case hex digit or a value of the wrong length.
    pub fn new<Text: Into<String>>(value: Text) -> Result<Self, ValueError> {
        let value = value.into();
        check_commit(&value)?;
        Ok(Self(value))
    }

    /// The hex digits.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn check_commit(value: &str) -> Result<(), ValueError> {
    if value.is_empty() {
        return Err(ValueError::Empty);
    }
    if let Some(found) = value.chars().find(|found| !is_hex(*found)) {
        return Err(ValueError::ForbiddenChar(found));
    }
    (7..=64)
        .contains(&value.len())
        .then_some(())
        .ok_or(ValueError::BadCommit)
}

impl fmt::Display for CommitSha {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Why a stack failed, shortened to a size a report can carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detail(String);

impl Detail {
    /// The text of `message`, cut to its last characters when it is long.
    #[must_use]
    pub fn new(message: &str) -> Self {
        let trimmed = message.trim();
        let extra = trimmed.chars().count().saturating_sub(MAX_DETAIL_CHARS);
        Self(trimmed.chars().skip(extra).collect())
    }

    /// The text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

collection!(
    /// The reserved `aegis/` stacks a host opts in to deploy.
    ReservedStacks,
    StackName
);

/// What a reconcile did with one stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Deployment {
    /// The stack was brought up.
    Running,
    /// Compose failed to bring the stack up.
    Failed(Detail),
    /// The stack broke a rule and was not deployed.
    Invalid(Violations),
}

/// The result for one stack directory, such as `hosts/jao-server/pihole`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackReport {
    directory: RepoPath,
    deployment: Deployment,
}

impl StackReport {
    /// The `deployment` of the stack in `directory`.
    #[must_use]
    pub const fn new(directory: RepoPath, deployment: Deployment) -> Self {
        Self {
            directory,
            deployment,
        }
    }

    /// The stack directory.
    #[must_use]
    pub const fn directory(&self) -> &RepoPath {
        &self.directory
    }

    /// What happened to it.
    #[must_use]
    pub const fn deployment(&self) -> &Deployment {
        &self.deployment
    }
}

collection!(
    /// The per-stack results of one reconcile.
    StackReports,
    StackReport
);

/// Which host reconciled which commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    host: HostName,
    commit: CommitSha,
}

impl Origin {
    /// The reconcile of `commit` on `host`.
    #[must_use]
    pub const fn new(host: HostName, commit: CommitSha) -> Self {
        Self { host, commit }
    }

    /// The host.
    #[must_use]
    pub const fn host(&self) -> &HostName {
        &self.host
    }

    /// The commit.
    #[must_use]
    pub const fn commit(&self) -> &CommitSha {
        &self.commit
    }
}

/// Everything a reconcile reports: where it ran and what happened to each stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    origin: Origin,
    stacks: StackReports,
}

impl Report {
    /// The `stacks` results of the reconcile at `origin`.
    #[must_use]
    pub const fn new(origin: Origin, stacks: StackReports) -> Self {
        Self { origin, stacks }
    }

    /// Where the reconcile ran.
    #[must_use]
    pub const fn origin(&self) -> &Origin {
        &self.origin
    }

    /// The per-stack results.
    #[must_use]
    pub const fn stacks(&self) -> &StackReports {
        &self.stacks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack(name: &str) -> StackName {
        StackName::new(name).unwrap()
    }

    #[test]
    fn a_project_is_the_stack_under_the_aegis_prefix() {
        let project = ProjectName::of(&stack("pihole")).unwrap();
        assert_eq!(project.as_str(), "aegis-pihole");
        assert_eq!(
            ProjectName::of(&stack("a_b-1")).unwrap().as_str(),
            "aegis-a_b-1"
        );
    }

    #[test]
    fn a_stack_name_compose_rejects_has_no_project() {
        assert_eq!(
            ProjectName::of(&stack("Web")),
            Err(ValueError::ForbiddenChar('W'))
        );
        assert_eq!(
            ProjectName::of(&stack("a.b")),
            Err(ValueError::ForbiddenChar('.'))
        );
    }

    #[test]
    fn a_commit_is_lower_case_hex() {
        assert!(CommitSha::new("0123abc").is_ok());
        assert!(CommitSha::new("a".repeat(40)).is_ok());
        assert_eq!(CommitSha::new(""), Err(ValueError::Empty));
        assert_eq!(
            CommitSha::new("ABCDEF1"),
            Err(ValueError::ForbiddenChar('A'))
        );
        assert_eq!(
            CommitSha::new("--help"),
            Err(ValueError::ForbiddenChar('-'))
        );
        assert_eq!(CommitSha::new("abc"), Err(ValueError::BadCommit));
        assert_eq!(CommitSha::new("a".repeat(65)), Err(ValueError::BadCommit));
    }

    #[test]
    fn a_long_detail_keeps_its_tail() {
        let long = format!("{}{}", "x".repeat(MAX_DETAIL_CHARS), "the end");
        let detail = Detail::new(&long);
        assert_eq!(detail.as_str().chars().count(), MAX_DETAIL_CHARS);
        assert!(detail.as_str().ends_with("the end"));
        assert_eq!(Detail::new("  short \n").as_str(), "short");
    }
}
