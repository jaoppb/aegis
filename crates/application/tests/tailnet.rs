//! The enrolment rule: a key is minted only while the sidecar has no state.

use std::future::{Future, ready};
use std::sync::atomic::{AtomicUsize, Ordering};

use aegis_application::{
    AuthKey, DeviceId, EnrollError, Enrolled, Enrollment, KeyMinter, Reason, SidecarState,
    StateVolume, TailnetError,
};
use aegis_domain::{HostName, TailnetTag, TailnetTags, ValueError};

struct Volume(Result<StateVolume, Reason>);

impl SidecarState for Volume {
    fn volume(&self, _node: &HostName) -> impl Future<Output = Result<StateVolume, Reason>> + Send {
        ready(self.0.clone())
    }
}

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

#[cfg(test)]
mod cases {
    use super::*;

    fn tags() -> TailnetTags {
        std::iter::once(TailnetTag::new("tag:app").unwrap()).collect()
    }

    fn node() -> HostName {
        HostName::new("app").unwrap()
    }

    fn block_on<Output>(future: impl Future<Output = Output>) -> Output {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        runtime.block_on(future)
    }

    fn enroll(
        volume: Result<StateVolume, Reason>,
        minter: &Counting,
    ) -> Result<Enrolled, EnrollError> {
        let enrollment = Enrollment::new(Volume(volume), minter);
        block_on(enrollment.enroll(&node(), &tags()))
    }

    #[test]
    fn an_empty_volume_mints_one_key() {
        let minter = Counting::default();
        let result = enroll(Ok(StateVolume::Empty), &minter);
        let key = AuthKey::new("tskey-auth-secret");
        assert_eq!(result, Ok(Enrolled::Minted(key)));
        assert_eq!(minter.mints.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn existing_state_mints_nothing() {
        let minter = Counting::default();
        let result = enroll(Ok(StateVolume::Populated), &minter);
        assert_eq!(result, Ok(Enrolled::AlreadyEnrolled));
        assert_eq!(minter.mints.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn unreadable_state_mints_nothing() {
        let minter = Counting::default();
        let result = enroll(Err(Reason::new(&"agent offline")), &minter);
        assert!(matches!(result, Err(EnrollError::State(_))));
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
}
