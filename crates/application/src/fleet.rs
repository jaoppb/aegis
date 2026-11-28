//! What the controller knows about the repository and the fleet: the commit it wants every host
//! at, what is wrong with the newest commit, and which hosts have confirmed the wanted commit.

use aegis_domain::{CommitSha, HostName, HostNames, Violations};

use crate::outcome::Outcome;
use crate::source::Reason;

/// The commit every host should converge to, and what loading it produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Goal {
    commit: CommitSha,
    outcome: Outcome,
}

impl Goal {
    /// The `outcome` of loading `commit`.
    #[must_use]
    pub const fn new(commit: CommitSha, outcome: Outcome) -> Self {
        Self { commit, outcome }
    }

    /// The commit.
    #[must_use]
    pub const fn commit(&self) -> &CommitSha {
        &self.commit
    }

    /// The stacks of the commit per host, and the stacks that were rejected.
    #[must_use]
    pub const fn outcome(&self) -> &Outcome {
        &self.outcome
    }
}

/// A commit whose tree could not be read at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid {
    commit: CommitSha,
    violations: Violations,
}

impl Invalid {
    /// `commit` is unreadable for `violations`.
    #[must_use]
    pub const fn new(commit: CommitSha, violations: Violations) -> Self {
        Self { commit, violations }
    }

    /// The commit.
    #[must_use]
    pub const fn commit(&self) -> &CommitSha {
        &self.commit
    }

    /// Why its tree cannot be read.
    #[must_use]
    pub const fn violations(&self) -> &Violations {
        &self.violations
    }
}

/// What one look at the repository found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observation {
    /// The newest commit loaded: it is the new goal.
    Loaded(Goal),
    /// The newest commit cannot be read at all.
    Unloadable(Invalid),
    /// The remote could not be fetched.
    Unreachable(Reason),
}

/// What is wrong at the moment, if anything. The goal is kept in every case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// The newest commit is the goal.
    Clear,
    /// The newest commit cannot be read, so the last good one stays the goal.
    Unloadable(Invalid),
    /// The remote could not be fetched, so nothing changed.
    Unreachable(Reason),
}

/// The hosts that still have to converge to a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Order {
    commit: CommitSha,
    hosts: HostNames,
}

impl Order {
    /// Tell `hosts` to converge to `commit`.
    #[must_use]
    pub const fn new(commit: CommitSha, hosts: HostNames) -> Self {
        Self { commit, hosts }
    }

    /// The commit.
    #[must_use]
    pub const fn commit(&self) -> &CommitSha {
        &self.commit
    }

    /// The hosts to tell.
    #[must_use]
    pub const fn hosts(&self) -> &HostNames {
        &self.hosts
    }
}

/// The agents the controller can tell to converge. Telling is fire and forget: an agent that
/// converges reports back, and the report reaches [`Fleet::acknowledge`].
pub trait Agents {
    /// The hosts that have an agent.
    fn hosts(&self) -> HostNames;

    /// Start telling every host of `order` to converge. A host already being told is skipped.
    fn converge(&self, order: &Order);
}

/// The controller's view: the goal, the notice and the hosts that confirmed the goal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fleet {
    goal: Option<Goal>,
    notice: Notice,
    confirmed: HostNames,
}

impl Default for Fleet {
    fn default() -> Self {
        Self {
            goal: None,
            notice: Notice::Clear,
            confirmed: HostNames::default(),
        }
    }
}

impl Fleet {
    /// Take in what the repository looked like. An unreadable commit or a failed fetch changes
    /// the notice and nothing else.
    pub fn observe(&mut self, observation: Observation) {
        match observation {
            Observation::Loaded(goal) => self.adopt(goal),
            Observation::Unloadable(invalid) => self.notice = Notice::Unloadable(invalid),
            Observation::Unreachable(reason) => self.notice = Notice::Unreachable(reason),
        }
    }

    fn adopt(&mut self, goal: Goal) {
        self.notice = Notice::Clear;
        let known = self.goal.as_ref().map(Goal::commit);
        let same = known == Some(goal.commit());
        self.confirmed = self.confirmed.iter().filter(|_| same).cloned().collect();
        self.goal = Some(goal);
    }

    /// Record that `host` reported `commit`. Only the goal's commit counts.
    pub fn acknowledge(&mut self, host: &HostName, commit: &CommitSha) {
        let wanted = self.goal.as_ref().map(Goal::commit);
        let counts = wanted == Some(commit) && !self.is_confirmed(host);
        let added = counts.then(|| host.clone());
        self.confirmed = self.confirmed.iter().cloned().chain(added).collect();
    }

    /// The hosts of `mapped` that have not confirmed the goal, or `None` when there is nothing to
    /// tell anybody.
    #[must_use]
    pub fn order(&self, mapped: &HostNames) -> Option<Order> {
        let goal = self.goal.as_ref()?;
        let pending: HostNames = mapped
            .iter()
            .filter(|host| !self.is_confirmed(host))
            .cloned()
            .collect();
        (!pending.is_empty()).then(|| Order::new(goal.commit().clone(), pending))
    }

    /// Whether `host` confirmed the goal.
    #[must_use]
    pub fn is_confirmed(&self, host: &HostName) -> bool {
        self.confirmed.iter().any(|known| known == host)
    }

    /// The commit every host should be at, once one has loaded.
    #[must_use]
    pub const fn goal(&self) -> Option<&Goal> {
        self.goal.as_ref()
    }

    /// What is wrong with the newest commit or the remote.
    #[must_use]
    pub const fn notice(&self) -> &Notice {
        &self.notice
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(text: &str) -> CommitSha {
        CommitSha::new(text).unwrap()
    }

    fn host(name: &str) -> HostName {
        HostName::new(name).unwrap()
    }

    fn goal(text: &str) -> Observation {
        Observation::Loaded(Goal::new(commit(text), Outcome::default()))
    }

    fn hosts(names: &[&str]) -> HostNames {
        names.iter().map(|name| host(name)).collect()
    }

    #[test]
    fn a_fleet_without_a_goal_orders_nobody() {
        assert!(Fleet::default().order(&hosts(&["a"])).is_none());
    }

    #[test]
    fn a_loaded_commit_becomes_the_goal_for_every_mapped_host() {
        let mut fleet = Fleet::default();
        fleet.observe(goal("aaaaaaa"));
        let order = fleet.order(&hosts(&["a", "b"])).unwrap();
        assert_eq!(order.commit(), &commit("aaaaaaa"));
        assert_eq!(order.hosts().len(), 2);
    }

    #[test]
    fn a_confirmed_host_is_not_told_again_until_the_goal_moves() {
        let mut fleet = Fleet::default();
        fleet.observe(goal("aaaaaaa"));
        fleet.acknowledge(&host("a"), &commit("aaaaaaa"));
        let order = fleet.order(&hosts(&["a", "b"])).unwrap();
        assert_eq!(order.hosts(), &hosts(&["b"]));
        fleet.observe(goal("bbbbbbb"));
        assert_eq!(fleet.order(&hosts(&["a"])).unwrap().hosts(), &hosts(&["a"]));
    }

    #[test]
    fn a_report_for_an_old_commit_does_not_confirm_the_goal() {
        let mut fleet = Fleet::default();
        fleet.observe(goal("bbbbbbb"));
        fleet.acknowledge(&host("a"), &commit("aaaaaaa"));
        assert!(!fleet.is_confirmed(&host("a")));
    }

    #[test]
    fn an_unreadable_commit_keeps_the_last_good_goal() {
        let mut fleet = Fleet::default();
        fleet.observe(goal("aaaaaaa"));
        let invalid = Invalid::new(commit("bbbbbbb"), Violations::default());
        fleet.observe(Observation::Unloadable(invalid.clone()));
        assert_eq!(fleet.goal().unwrap().commit(), &commit("aaaaaaa"));
        assert_eq!(fleet.notice(), &Notice::Unloadable(invalid));
    }

    #[test]
    fn a_failed_fetch_keeps_the_goal_and_the_confirmations() {
        let mut fleet = Fleet::default();
        fleet.observe(goal("aaaaaaa"));
        fleet.acknowledge(&host("a"), &commit("aaaaaaa"));
        fleet.observe(Observation::Unreachable(Reason::new(&"offline")));
        assert!(fleet.is_confirmed(&host("a")));
        assert!(matches!(fleet.notice(), Notice::Unreachable(_)));
        fleet.observe(goal("aaaaaaa"));
        assert_eq!(fleet.notice(), &Notice::Clear);
        assert!(fleet.is_confirmed(&host("a")));
    }
}
