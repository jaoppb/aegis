//! The controller's enrolment: it names the tailnet once, mints only for a node without state and
//! only for tags the operator allows.

use std::future::{Future, ready};
use std::sync::atomic::{AtomicUsize, Ordering};

use aegis_application::{
    AuthKey, EnrollError, Enrolled, Enroller, EnrollmentRequest, KeyMinter, SidecarEnroller,
    StateVolume, TagError, TailnetError, TailnetNames,
};
use aegis_domain::{NodeLabel, TailnetName, TailnetTag, TailnetTags};

#[derive(Default)]
#[cfg(test)]
struct Names {
    calls: AtomicUsize,
}

#[cfg(test)]
impl TailnetNames for &Names {
    fn tailnet(&self) -> impl Future<Output = Result<TailnetName, TailnetError>> + Send {
        self.calls.fetch_add(1, Ordering::SeqCst);
        ready(Ok(TailnetName::new("taildfab0.ts.net").unwrap()))
    }
}

#[derive(Default)]
#[cfg(test)]
struct Minter {
    mints: AtomicUsize,
}

#[cfg(test)]
impl KeyMinter for &Minter {
    fn mint(
        &self,
        _tags: &TailnetTags,
    ) -> impl Future<Output = Result<AuthKey, TailnetError>> + Send {
        self.mints.fetch_add(1, Ordering::SeqCst);
        ready(Ok(AuthKey::new("tskey-auth-secret")))
    }
}

#[cfg(test)]
fn tags(names: &[&str]) -> TailnetTags {
    names
        .iter()
        .map(|name| TailnetTag::new(*name).unwrap())
        .collect()
}

#[cfg(test)]
fn request(state: StateVolume, wanted: &[&str]) -> EnrollmentRequest {
    EnrollmentRequest::new(NodeLabel::new("vaultwarden").unwrap(), tags(wanted), state)
}

#[cfg(test)]
fn block_on<Output>(future: impl Future<Output = Output>) -> Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(future)
}

#[test]
fn a_node_without_state_gets_the_tailnet_and_one_key() {
    let (names, minter) = (Names::default(), Minter::default());
    let enroller = Enroller::new(&names, &minter, tags(&["tag:app"]));
    let grant = block_on(enroller.enroll(&request(StateVolume::Empty, &["tag:app"]))).unwrap();
    assert_eq!(grant.tailnet().as_str(), "taildfab0.ts.net");
    assert!(matches!(grant.enrolled(), Enrolled::Minted(_)));
    assert_eq!(minter.mints.load(Ordering::SeqCst), 1);
}

#[test]
fn a_node_with_state_gets_the_tailnet_and_no_key() {
    let (names, minter) = (Names::default(), Minter::default());
    let enroller = Enroller::new(&names, &minter, tags(&["tag:app"]));
    let grant = block_on(enroller.enroll(&request(StateVolume::Populated, &["tag:app"]))).unwrap();
    assert_eq!(grant.enrolled(), &Enrolled::AlreadyEnrolled);
    assert_eq!(minter.mints.load(Ordering::SeqCst), 0);
}

#[test]
fn the_tailnet_is_discovered_once() {
    let (names, minter) = (Names::default(), Minter::default());
    let enroller = Enroller::new(&names, &minter, tags(&["tag:app"]));
    let asked = request(StateVolume::Populated, &["tag:app"]);
    block_on(enroller.enroll(&asked)).unwrap();
    block_on(enroller.enroll(&asked)).unwrap();
    assert_eq!(names.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn a_tag_the_operator_did_not_allow_mints_nothing() {
    let (names, minter) = (Names::default(), Minter::default());
    let enroller = Enroller::new(&names, &minter, tags(&["tag:app"]));
    let result = block_on(enroller.enroll(&request(StateVolume::Empty, &["tag:admin"])));
    let refused = TailnetTag::new("tag:admin").unwrap();
    assert_eq!(
        result,
        Err(EnrollError::Tags(TagError::NotAllowed(refused)))
    );
    assert_eq!(minter.mints.load(Ordering::SeqCst), 0);
}

#[test]
fn an_aegis_role_tag_is_refused_even_when_allowed() {
    let (names, minter) = (Names::default(), Minter::default());
    let enroller = Enroller::new(&names, &minter, tags(&["tag:aegis-agent"]));
    let result = block_on(enroller.enroll(&request(StateVolume::Empty, &["tag:aegis-agent"])));
    assert!(matches!(
        result,
        Err(EnrollError::Tags(TagError::Reserved(_)))
    ));
    assert_eq!(minter.mints.load(Ordering::SeqCst), 0);
}
