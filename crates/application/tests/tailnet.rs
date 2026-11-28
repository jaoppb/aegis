//! The enrolment rule: a key is minted only while the sidecar has no state.

use std::future::{Future, ready};
use std::sync::atomic::{AtomicUsize, Ordering};

use aegis_application::{
    AuthKey, DeviceId, EnrollError, Enrolled, Enrollment, KeyMinter, Reason, StateVolume, TagError,
    TailnetError,
};
use aegis_domain::{TailnetTag, TailnetTags, ValueError};

#[derive(Default)]
struct Counting {
    mints: AtomicUsize,
}

impl KeyMinter for &Counting {
    fn mint(
        &self,
        _tags: &TailnetTags,
    ) -> impl Future<Output = Result<AuthKey, TailnetError>> + Send {
        self.mints.fetch_add(1, Ordering::SeqCst);
        ready(Ok(AuthKey::new("tskey-auth-secret")))
    }
}

struct Failing;

impl KeyMinter for Failing {
    fn mint(
        &self,
        _tags: &TailnetTags,
    ) -> impl Future<Output = Result<AuthKey, TailnetError>> + Send {
        ready(Err(TailnetError::Rejected(Reason::new(&"403"))))
    }
}

#[cfg(test)]
mod cases {
    use super::*;

    fn tags() -> TailnetTags {
        std::iter::once(TailnetTag::new("tag:app").unwrap()).collect()
    }

    fn block_on<Output>(future: impl Future<Output = Output>) -> Output {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        runtime.block_on(future)
    }

    fn enroll(volume: StateVolume, minter: &Counting) -> Result<Enrolled, EnrollError> {
        let enrollment = Enrollment::new(minter);
        block_on(enrollment.enroll(&tags(), volume))
    }

    #[test]
    fn an_empty_volume_mints_one_key() {
        let minter = Counting::default();
        let result = enroll(StateVolume::Empty, &minter);
        let key = AuthKey::new("tskey-auth-secret");
        assert_eq!(result, Ok(Enrolled::Minted(key)));
        assert_eq!(minter.mints.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn existing_state_mints_nothing() {
        let minter = Counting::default();
        let result = enroll(StateVolume::Populated, &minter);
        assert_eq!(result, Ok(Enrolled::AlreadyEnrolled));
        assert_eq!(minter.mints.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_key_never_prints_its_value() {
        let key = AuthKey::new("tskey-auth-secret");
        assert!(!format!("{key:?}").contains("secret"));
    }

    #[test]
    fn a_device_id_cannot_alter_a_path() {
        assert!(DeviceId::new("12345-abc").is_ok());
        assert_eq!(DeviceId::new(""), Err(ValueError::Empty));
        let bad = ["a/b", "..", "a?b", "a#b", "a b"];
        assert!(bad.iter().all(|text| DeviceId::new(*text).is_err()));
    }

    fn enroll_tagged(names: &[&str], minter: &Counting) -> Result<Enrolled, EnrollError> {
        let chosen: TailnetTags = names
            .iter()
            .map(|name| TailnetTag::new(*name).unwrap())
            .collect();
        let enrollment = Enrollment::new(minter);
        block_on(enrollment.enroll(&chosen, StateVolume::Empty))
    }

    #[test]
    fn a_node_needs_a_tag() {
        let minter = Counting::default();
        let result = enroll_tagged(&[], &minter);
        assert_eq!(result, Err(EnrollError::Tags(TagError::Empty)));
        assert_eq!(minter.mints.load(Ordering::SeqCst), 0);
    }

    fn claim(reserved: &str) {
        let minter = Counting::default();
        let result = enroll_tagged(&["tag:app", reserved], &minter);
        let tag = TailnetTag::new(reserved).unwrap();
        assert_eq!(result, Err(EnrollError::Tags(TagError::Reserved(tag))));
        assert_eq!(minter.mints.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_stack_cannot_claim_the_controller_tag() {
        claim("tag:aegis-controller");
    }

    #[test]
    fn a_stack_cannot_claim_the_agent_tag() {
        claim("tag:aegis-agent");
    }

    #[test]
    fn a_failed_mint_is_reported() {
        let enrollment = Enrollment::new(Failing);
        let result = block_on(enrollment.enroll(&tags(), StateVolume::Empty));
        assert!(matches!(result, Err(EnrollError::Mint(_))));
    }
}
