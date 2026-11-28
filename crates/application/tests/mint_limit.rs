//! The mint limiter counts keys in a sliding window per stack, per host and in total.

#[cfg(test)]
mod cases {
    use std::num::NonZeroU32;
    use std::time::{Duration, Instant};

    use aegis_application::{Allowance, MintLimiter, MintLimits, Minting, Scope};
    use aegis_domain::{HostName, StackName};
    fn allowance(count: u32) -> Allowance {
        Allowance::new(NonZeroU32::new(count).unwrap())
    }

    fn minting(host: &str, stack: &str) -> Minting {
        Minting::new(HostName::new(host).unwrap(), StackName::new(stack).unwrap())
    }

    fn limiter() -> MintLimiter {
        let limits = MintLimits::new(allowance(2), allowance(3), allowance(4));
        MintLimiter::new(limits.over(Duration::from_secs(100)))
    }

    #[test]
    fn keys_within_every_allowance_are_counted() {
        let mut limiter = limiter();
        let now = Instant::now();
        assert!(limiter.reserve(minting("a", "x"), now).is_ok());
        assert!(limiter.reserve(minting("a", "x"), now).is_ok());
        assert!(limiter.reserve(minting("a", "y"), now).is_ok());
    }

    #[test]
    fn a_stack_over_its_allowance_is_refused_while_others_are_not() {
        let mut limiter = limiter();
        let now = Instant::now();
        limiter.reserve(minting("a", "x"), now).unwrap();
        limiter.reserve(minting("a", "x"), now).unwrap();
        let refused = limiter.reserve(minting("a", "x"), now).unwrap_err();
        assert_eq!(refused.scope(), Scope::Stack);
        assert!(limiter.reserve(minting("a", "y"), now).is_ok());
    }

    #[test]
    fn a_host_over_its_allowance_is_refused_for_every_stack() {
        let mut limiter = limiter();
        let now = Instant::now();
        let stacks = ["x", "y", "z"].map(|stack| limiter.reserve(minting("a", stack), now));
        assert!(stacks.iter().all(Result::is_ok));
        let refused = limiter.reserve(minting("a", "w"), now).unwrap_err();
        assert_eq!(refused.scope(), Scope::Host);
        assert!(limiter.reserve(minting("b", "w"), now).is_ok());
    }

    #[test]
    fn the_total_is_refused_across_hosts() {
        let mut limiter = limiter();
        let now = Instant::now();
        let hosts = ["a", "b", "c", "d"].map(|host| limiter.reserve(minting(host, "x"), now));
        assert!(hosts.iter().all(Result::is_ok));
        let refused = limiter.reserve(minting("e", "x"), now).unwrap_err();
        assert_eq!(refused.scope(), Scope::Total);
    }

    #[test]
    fn a_refusal_says_when_the_oldest_key_leaves_the_window() {
        let mut limiter = limiter();
        let start = Instant::now();
        limiter.reserve(minting("a", "x"), start).unwrap();
        let later = start + Duration::from_secs(30);
        limiter.reserve(minting("a", "x"), later).unwrap();
        let refused = limiter.reserve(minting("a", "x"), later).unwrap_err();
        assert_eq!(refused.retry_after(), Duration::from_secs(70));
    }

    #[test]
    fn keys_leave_the_window_and_minting_resumes() {
        let mut limiter = limiter();
        let start = Instant::now();
        limiter.reserve(minting("a", "x"), start).unwrap();
        limiter.reserve(minting("a", "x"), start).unwrap();
        let after = start + Duration::from_secs(100);
        assert!(limiter.reserve(minting("a", "x"), after).is_ok());
    }

    #[test]
    fn a_refused_key_is_not_counted() {
        let mut limiter = limiter();
        let start = Instant::now();
        limiter.reserve(minting("a", "x"), start).unwrap();
        limiter.reserve(minting("a", "x"), start).unwrap();
        (0..5).for_each(|_| assert!(limiter.reserve(minting("a", "x"), start).is_err()));
        let after = start + Duration::from_secs(100);
        assert!(limiter.reserve(minting("a", "x"), after).is_ok());
        assert!(limiter.reserve(minting("a", "x"), after).is_ok());
    }
}
