//! A cap on the keys the controller mints for the agents, so one compromised agent cannot drain
//! the tailnet's key budget: a sliding window per host and stack, per host and in total.

use std::fmt;
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use aegis_domain::{HostName, StackName};

/// The window the limits count in, unless [`MintLimits::over`] says another.
const STANDARD_WINDOW: Duration = Duration::from_hours(1);

/// How many keys may be minted in the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Allowance(NonZeroU32);

impl Allowance {
    /// An allowance of `count` keys.
    #[must_use]
    pub const fn new(count: NonZeroU32) -> Self {
        Self(count)
    }

    fn reached_by(self, minted: usize) -> bool {
        let count = u32::try_from(minted).unwrap_or(u32::MAX);
        count >= self.0.get()
    }
}

/// The allowances per host and stack, per host and in total, and the window they count in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MintLimits {
    per_stack: Allowance,
    per_host: Allowance,
    total: Allowance,
    window: Duration,
}

impl MintLimits {
    /// The allowances `per_stack`, `per_host` and `total`, over one hour.
    #[must_use]
    pub const fn new(per_stack: Allowance, per_host: Allowance, total: Allowance) -> Self {
        Self {
            per_stack,
            per_host,
            total,
            window: STANDARD_WINDOW,
        }
    }

    /// The limits a controller applies unless told otherwise: 3 keys per stack of a host, 20 per
    /// host and 50 in total, over one hour.
    #[must_use]
    pub const fn standard() -> Self {
        let per_stack = NonZeroU32::MIN.saturating_add(2);
        let per_host = NonZeroU32::MIN.saturating_add(19);
        let total = NonZeroU32::MIN.saturating_add(49);
        Self::new(
            Allowance::new(per_stack),
            Allowance::new(per_host),
            Allowance::new(total),
        )
    }

    /// The same limits over `window`.
    #[must_use]
    pub const fn over(self, window: Duration) -> Self {
        Self { window, ..self }
    }

    /// The allowance per stack of a host.
    #[must_use]
    pub const fn per_stack(&self) -> Allowance {
        self.per_stack
    }

    /// The allowance per host.
    #[must_use]
    pub const fn per_host(&self) -> Allowance {
        self.per_host
    }

    /// The allowance in total.
    #[must_use]
    pub const fn total(&self) -> Allowance {
        self.total
    }

    /// The window the allowances count in.
    #[must_use]
    pub const fn window(&self) -> Duration {
        self.window
    }

    const fn allowance(&self, scope: Scope) -> Allowance {
        match scope {
            Scope::Stack => self.per_stack,
            Scope::Host => self.per_host,
            Scope::Total => self.total,
        }
    }
}

/// The count a limit applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The keys for one stack of one host.
    Stack,
    /// The keys for one host.
    Host,
    /// Every key.
    Total,
}

impl fmt::Display for Scope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Stack => "for this stack",
            Self::Host => "for this host",
            Self::Total => "in total",
        })
    }
}

/// Who a key is minted for: a stack of a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Minting {
    host: HostName,
    stack: StackName,
}

impl Minting {
    /// A key for the stack `stack` of `host`.
    #[must_use]
    pub const fn new(host: HostName, stack: StackName) -> Self {
        Self { host, stack }
    }

    fn within(&self, scope: Scope, other: &Self) -> bool {
        match scope {
            Scope::Stack => self == other,
            Scope::Host => self.host == other.host,
            Scope::Total => true,
        }
    }
}

/// A limit was reached: which one, and how long until a key may be minted again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exceeded {
    scope: Scope,
    retry_after: Duration,
}

impl Exceeded {
    /// The limit that was reached.
    #[must_use]
    pub const fn scope(&self) -> Scope {
        self.scope
    }

    /// How long until the oldest key counted falls out of the window.
    #[must_use]
    pub const fn retry_after(&self) -> Duration {
        self.retry_after
    }
}

impl fmt::Display for Exceeded {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "too many keys minted {}", self.scope)
    }
}

impl std::error::Error for Exceeded {}

#[derive(Debug, Clone)]
struct Minted {
    minting: Minting,
    seen: Instant,
}

/// The keys minted in the window, oldest first.
#[derive(Debug, Clone, Default)]
struct MintLog(Vec<Minted>);

impl MintLog {
    fn forget_before(&mut self, now: Instant, window: Duration) {
        let recent = |minted: &Minted| now.saturating_duration_since(minted.seen) < window;
        self.0.retain(recent);
    }

    fn within<'log>(
        &'log self,
        scope: Scope,
        minting: &'log Minting,
    ) -> impl Iterator<Item = &'log Minted> {
        let counted = move |minted: &&Minted| minting.within(scope, &minted.minting);
        self.0.iter().filter(counted)
    }

    fn oldest(&self, scope: Scope, minting: &Minting) -> Option<Instant> {
        self.within(scope, minting).next().map(|minted| minted.seen)
    }
}

/// Counts the keys minted and refuses one beyond the limits. It keeps nothing on disk: a restart
/// forgets the count, and the log holds at most the total allowance.
#[derive(Debug, Clone)]
pub struct MintLimiter {
    limits: MintLimits,
    log: MintLog,
}

impl MintLimiter {
    /// A limiter that has seen no key and applies `limits`.
    #[must_use]
    pub fn new(limits: MintLimits) -> Self {
        Self {
            limits,
            log: MintLog::default(),
        }
    }

    /// Count a key for `minting` at `now`, unless a limit is reached. The key is counted before
    /// it is minted, so a mint that fails still counts and a failing controller is not hammered.
    ///
    /// # Errors
    ///
    /// Returns the [`Exceeded`] limit that is reached first, stack and host before total.
    pub fn reserve(&mut self, minting: Minting, now: Instant) -> Result<(), Exceeded> {
        let window = self.limits.window;
        self.log.forget_before(now, window);
        let reached = self.reached(&minting);
        reached.map_or(Ok(()), |limit| Err(limit.exceeded(window, now)))?;
        self.log.0.push(Minted { minting, seen: now });
        Ok(())
    }

    fn reached(&self, minting: &Minting) -> Option<Reached> {
        let scopes = [Scope::Stack, Scope::Host, Scope::Total];
        let scope = scopes
            .into_iter()
            .find(|scope| self.is_full(*scope, minting))?;
        let oldest = self.log.oldest(scope, minting);
        Some(Reached { scope, oldest })
    }

    fn is_full(&self, scope: Scope, minting: &Minting) -> bool {
        let minted = self.log.within(scope, minting).count();
        self.limits.allowance(scope).reached_by(minted)
    }
}

/// A limit that is reached, and when the oldest key it counts was minted.
struct Reached {
    scope: Scope,
    oldest: Option<Instant>,
}

impl Reached {
    fn exceeded(self, window: Duration, now: Instant) -> Exceeded {
        let since = self.oldest.map(|seen| now.saturating_duration_since(seen));
        Exceeded {
            scope: self.scope,
            retry_after: window.saturating_sub(since.unwrap_or_default()),
        }
    }
}
