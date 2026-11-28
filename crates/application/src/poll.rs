//! Looks at the repository: fetches the newest commit, loads and validates it.

use aegis_domain::{RepoPath, Violation, Violations};

use crate::deploy::Upstream;
use crate::fleet::{Goal, Invalid, Observation};
use crate::outcome::Outcome;
use crate::reconcile::Repository;
use crate::source::Reason;
use aegis_domain::CommitSha;

/// Follows the remote branch and reads each new commit.
#[derive(Debug)]
pub struct Poller<Up> {
    upstream: Up,
    repository: Repository,
}

impl<Up> Poller<Up> {
    /// A poller that fetches through `upstream` and reads the checkout through `repository`.
    #[must_use]
    pub const fn new(upstream: Up, repository: Repository) -> Self {
        Self {
            upstream,
            repository,
        }
    }
}

impl<Up: Upstream + Sync> Poller<Up> {
    /// Fetch the newest commit and load it. A commit with rejected stacks is still a goal: only a
    /// tree that cannot be read at all is unloadable.
    pub async fn observe(&self) -> Observation {
        match self.upstream.advance().await {
            Ok(commit) => self.load(commit),
            Err(error) => Observation::Unreachable(Reason::new(&error)),
        }
    }

    fn load(&self, commit: CommitSha) -> Observation {
        let outcome = self.repository.load();
        match unreadable_roots(&outcome) {
            Some(violations) => Observation::Unloadable(Invalid::new(commit, violations)),
            None => Observation::Loaded(Goal::new(commit, outcome)),
        }
    }
}

/// The violations of the `hosts` and `aegis` directories themselves, when either is unreadable.
fn unreadable_roots(outcome: &Outcome) -> Option<Violations> {
    let roots = [RepoPath::hosts_root(), RepoPath::reserved_root()];
    let rejections = outcome.rejected().iter();
    let at_root = rejections.filter(|rejection| roots.contains(rejection.directory()));
    let found: Vec<Violation> = at_root
        .flat_map(|rejection| rejection.violations().iter().cloned())
        .collect();
    (!found.is_empty()).then(|| Violations::new(found))
}
