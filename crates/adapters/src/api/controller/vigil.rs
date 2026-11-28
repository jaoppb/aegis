//! What the controller remembers of the requests for nodes: how many keys it minted, and which
//! requests it refused. Both live in memory and are forgotten when the controller restarts.

use std::time::Instant;

use aegis_api::version_1::{Counted, RefusalReason, Refused};
use aegis_application::{MintLimiter, MintLimits};
use aegis_domain::{HostName, StackName};

/// The most kinds of refusal the status lists; when more arrive, the one seen longest ago is
/// forgotten.
const MAX_ENTRIES: usize = 50;

/// The most kinds of refusal one host keeps in the status, so a host that makes up many cannot push
/// the refusals of the others out.
const MAX_PER_HOST: usize = 10;

/// Who asked for what was refused: the host of the agent and, when the request named a valid
/// one, the stack.
#[derive(Debug, Clone, Copy)]
pub(super) struct Who<'asker> {
    host: &'asker HostName,
    stack: Option<&'asker StackName>,
}

impl<'asker> Who<'asker> {
    pub(super) const fn new(host: &'asker HostName, stack: Option<&'asker StackName>) -> Self {
        Self { host, stack }
    }
}

/// A kind of refusal: who asked, for which stack, and why it was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Kind {
    host: HostName,
    stack: Option<StackName>,
    reason: RefusalReason,
}

#[derive(Debug)]
struct Entry {
    kind: Kind,
    count: u32,
    last: Instant,
}

impl Entry {
    fn refused(&self, now: Instant) -> Refused {
        let ago = now.saturating_duration_since(self.last).as_secs();
        let times = Counted::new(self.count, ago);
        let refused = Refused::new(self.kind.host.as_str(), self.kind.reason, times);
        match &self.kind.stack {
            Some(stack) => refused.for_stack(stack.as_str()),
            None => refused,
        }
    }
}

/// The refusals the status lists, the kinds seen most recently last.
#[derive(Debug, Default)]
struct Refusals(Vec<Entry>);

impl Refusals {
    /// Forget the oldest kind of the host that was refused last when it has more than
    /// [`MAX_PER_HOST`].
    fn trim_host_of_last(&mut self) {
        let asker = self.0.last().map(|entry| entry.kind.host.clone());
        let theirs = |entry: &Entry| asker.as_ref() == Some(&entry.kind.host);
        let surplus = self.0.iter().filter(|entry| theirs(entry)).count() > MAX_PER_HOST;
        let oldest = self.0.iter().position(theirs);
        drop(oldest.filter(|_| surplus).map(|index| self.0.remove(index)));
    }

    fn note(&mut self, kind: Kind, now: Instant) {
        let position = self.0.iter().position(|entry| entry.kind == kind);
        let mut entry = match position {
            Some(index) => self.0.remove(index),
            None => Entry {
                kind,
                count: 0,
                last: now,
            },
        };
        entry.count = entry.count.saturating_add(1);
        entry.last = now;
        self.0.push(entry);
        self.trim_host_of_last();
        let excess = self.0.len().saturating_sub(MAX_ENTRIES);
        drop(self.0.drain(..excess));
    }
}

/// The key counter and the refusals of the controller.
#[derive(Debug)]
pub(super) struct Vigil {
    limiter: MintLimiter,
    refusals: Refusals,
}

impl Vigil {
    pub(super) fn new(limits: MintLimits) -> Self {
        Self {
            limiter: MintLimiter::new(limits),
            refusals: Refusals::default(),
        }
    }

    pub(super) const fn limiter(&mut self) -> &mut MintLimiter {
        &mut self.limiter
    }

    /// Remember that the request of `who` was refused for `reason`.
    pub(super) fn refuse(&mut self, who: Who<'_>, reason: RefusalReason) {
        let kind = Kind {
            host: who.host.clone(),
            stack: who.stack.cloned(),
            reason,
        };
        self.refusals.note(kind, Instant::now());
    }

    /// The refusals of every host, or only those of `host`.
    pub(super) fn refused(&self, host: Option<&HostName>) -> Vec<Refused> {
        let now = Instant::now();
        let wanted = |entry: &&Entry| host.is_none_or(|only| *only == entry.kind.host);
        let kept = self.refusals.0.iter().filter(wanted);
        kept.map(|entry| entry.refused(now)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(name: &str) -> HostName {
        HostName::new(name).unwrap()
    }

    fn stack(name: &str) -> StackName {
        StackName::new(name).unwrap()
    }

    fn vigil() -> Vigil {
        Vigil::new(MintLimits::standard())
    }

    fn refuse_stack(vigil: &mut Vigil, name: &str) {
        let asker = host("h");
        let stack = stack(name);
        vigil.refuse(Who::new(&asker, Some(&stack)), RefusalReason::NotPlaced);
    }

    #[test]
    fn the_same_refusal_is_one_entry_that_counts_up() {
        let mut vigil = vigil();
        refuse_stack(&mut vigil, "a");
        refuse_stack(&mut vigil, "a");
        let listed = vigil.refused(None);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].times().count(), 2);
    }

    fn refuse_for(vigil: &mut Vigil, asker: &str, name: &str) {
        let asker = host(asker);
        let stack = stack(name);
        vigil.refuse(Who::new(&asker, Some(&stack)), RefusalReason::NotPlaced);
    }

    #[test]
    fn the_refusal_seen_longest_ago_is_forgotten_beyond_fifty_kinds() {
        let mut vigil = vigil();
        let askers: Vec<String> = (0..=MAX_ENTRIES).map(|each| format!("h{each}")).collect();
        askers
            .iter()
            .fold((), |(), asker| refuse_for(&mut vigil, asker, "s"));
        let listed = vigil.refused(None);
        assert_eq!(listed.len(), MAX_ENTRIES);
        assert_eq!(listed[0].host(), "h1");
    }

    #[test]
    fn a_repeated_refusal_is_refreshed_and_outlives_older_ones() {
        let mut vigil = vigil();
        let askers: Vec<String> = (0..MAX_ENTRIES - 1)
            .map(|each| format!("h{each}"))
            .collect();
        refuse_for(&mut vigil, "first", "s");
        askers
            .iter()
            .fold((), |(), asker| refuse_for(&mut vigil, asker, "s"));
        refuse_for(&mut vigil, "first", "s");
        refuse_for(&mut vigil, "newest", "s");
        let listed = vigil.refused(None);
        let first = listed.iter().find(|each| each.host() == "first");
        assert_eq!(first.unwrap().times().count(), 2);
        assert_eq!(listed.len(), MAX_ENTRIES);
    }

    #[test]
    fn one_host_making_up_stack_names_cannot_push_out_another_hosts_refusals() {
        let mut vigil = vigil();
        refuse_for(&mut vigil, "quiet", "s");
        let names: Vec<String> = (0..3 * MAX_ENTRIES)
            .map(|each| format!("s{each}"))
            .collect();
        names
            .iter()
            .fold((), |(), name| refuse_for(&mut vigil, "noisy", name));
        let quiet = vigil.refused(Some(&host("quiet")));
        assert_eq!(quiet.len(), 1);
        assert_eq!(vigil.refused(Some(&host("noisy"))).len(), MAX_PER_HOST);
    }

    #[test]
    fn one_host_sees_only_its_own_refusals() {
        let mut vigil = vigil();
        refuse_stack(&mut vigil, "a");
        let other = host("g");
        vigil.refuse(Who::new(&other, None), RefusalReason::NotPurged);
        assert_eq!(vigil.refused(Some(&other)).len(), 1);
        assert_eq!(vigil.refused(None).len(), 2);
    }
}
