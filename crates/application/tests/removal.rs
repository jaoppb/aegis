//! The rule that decides which absolute paths a purge may delete, and who may be purged.

use aegis_application::{Refusal, SafePath};
use aegis_domain::{
    ComposeSource, ComposeSummary, HostPath, HostPaths, RepoPath, Stack, StackDefinition,
    StackIdentity, StackKind, StackName, StackPolicy, StackState, TailnetTags, Tombstone,
    TombstoneError,
};

#[cfg(test)]
fn path(text: &str) -> HostPath {
    HostPath::new(text).unwrap()
}

#[cfg(test)]
fn taken(paths: &[&str]) -> HostPaths {
    HostPaths::new(paths.iter().map(|text| path(text)).collect())
}

#[cfg(test)]
fn check(candidate: &str, others: &[&str]) -> Result<SafePath, Refusal> {
    SafePath::check(&path(candidate), &taken(others))
}

#[test]
fn a_deep_plain_unshared_path_may_be_deleted() {
    assert!(check("/srv/data/web", &[]).is_ok());
}

#[test]
fn the_root_and_top_level_directories_are_too_shallow() {
    for shallow in ["/", "/srv", "/home/jao"] {
        assert!(
            matches!(
                check(shallow, &[]),
                Err(Refusal::TooShallow(_) | Refusal::Unusable(_))
            ),
            "{shallow}"
        );
    }
}

#[test]
fn a_path_with_dot_or_empty_segments_or_a_colon_is_unusable() {
    for odd in [
        "/srv/./data/web",
        "/srv//data/web",
        "/srv/data/web/",
        "/srv/da:ta/web",
    ] {
        assert!(
            matches!(check(odd, &[]), Err(Refusal::Unusable(_))),
            "{odd}"
        );
    }
}

#[test]
fn a_path_equal_to_a_taken_path_is_shared() {
    let refused = check("/srv/data/web", &["/srv/data/web"]);
    assert!(matches!(refused, Err(Refusal::Shared(_))));
}

#[test]
fn a_path_above_or_below_a_taken_path_is_shared() {
    assert!(matches!(
        check("/srv/data/web", &["/srv/data/web/uploads"]),
        Err(Refusal::Shared(_))
    ));
    assert!(matches!(
        check("/srv/data/web/uploads", &["/srv/data/web"]),
        Err(Refusal::Shared(_))
    ));
}

#[test]
fn a_taken_path_written_with_a_trailing_or_doubled_slash_still_overlaps() {
    for taken_path in ["/srv/data/web/", "/srv//data/web", "/srv/./data/web"] {
        let refused = check("/srv/data/web/uploads", &[taken_path]);
        assert!(matches!(refused, Err(Refusal::Shared(_))), "{taken_path}");
    }
}

#[test]
fn system_locations_are_refused_even_when_deep() {
    for system in [
        "/var/run/docker.sock",
        "/var/lib/docker/volumes",
        "/etc/ssl/certs",
        "/dev/dri/renderD128",
        "/run/user/1000/x",
        "/usr/local/share",
    ] {
        assert!(
            matches!(check(system, &[]), Err(Refusal::System(_))),
            "{system}"
        );
    }
}

#[test]
fn lookalikes_of_system_locations_are_not_refused() {
    for fine in [
        "/etcetera/a/b",
        "/var/lib/dockerx/a",
        "/usr2/a/b",
        "/var/lib/other/x",
    ] {
        assert!(check(fine, &[]).is_ok(), "{fine}");
    }
}

#[cfg(test)]
const SYSTEM: [&str; 17] = [
    "/bin",
    "/boot",
    "/dev",
    "/etc",
    "/lib",
    "/proc",
    "/root",
    "/run",
    "/sbin",
    "/sys",
    "/usr",
    "/var/lib/docker",
    "/var/run",
    "/lib64",
    "/nix",
    "/var/lib/containerd",
    "/var/lib/containers",
];

#[test]
fn every_system_location_is_refused() {
    for location in SYSTEM {
        let below = format!("{location}/a/b");
        assert!(
            matches!(check(&below, &[]), Err(Refusal::System(_))),
            "{below}"
        );
    }
}

#[test]
fn a_path_with_a_comma_is_unusable() {
    assert!(matches!(
        check("/srv/da,ta/web", &[]),
        Err(Refusal::Unusable(_))
    ));
}

#[test]
fn a_sibling_whose_name_starts_the_same_is_not_shared() {
    assert!(check("/srv/data/web", &["/srv/data/web2"]).is_ok());
}

#[cfg(test)]
fn stack(kind: StackKind, state: StackState) -> Stack {
    let policy = StackPolicy::new(None, TailnetTags::default(), state);
    let identity = StackIdentity::new(StackName::new("old").unwrap(), kind);
    let definition = StackDefinition::new(
        policy,
        ComposeSource::new("services: {}\n"),
        ComposeSummary::default(),
    );
    Stack::new(identity, definition)
}

#[cfg(test)]
fn directory() -> RepoPath {
    RepoPath::new("hosts/host/old").unwrap()
}

#[test]
fn only_a_purged_user_stack_has_a_tombstone() {
    let purged = stack(StackKind::Host, StackState::Purged);
    assert!(Tombstone::of(directory(), &purged).is_ok());
    let active = stack(StackKind::Host, StackState::Active);
    assert_eq!(
        Tombstone::of(directory(), &active),
        Err(TombstoneError::NotPurged)
    );
}

#[test]
fn a_reserved_stack_never_has_a_tombstone() {
    let reserved = stack(StackKind::Reserved, StackState::Purged);
    assert_eq!(
        Tombstone::of(directory(), &reserved),
        Err(TombstoneError::Reserved)
    );
}

#[test]
fn a_tombstone_names_the_project_of_its_stack() {
    let purged = stack(StackKind::Host, StackState::Purged);
    let tombstone = Tombstone::of(directory(), &purged).unwrap();
    assert_eq!(tombstone.project().as_str(), "aegis-old");
    assert!(tombstone.node().is_none());
    assert!(tombstone.paths().is_empty());
}

#[test]
fn a_purged_stack_with_a_node_names_the_node_to_delete() {
    let node = aegis_domain::Node::new(aegis_domain::ServiceName::new("files").unwrap());
    let policy = StackPolicy::new(None, TailnetTags::default(), StackState::Purged).with_node(node);
    let identity = StackIdentity::new(StackName::new("old").unwrap(), StackKind::Host);
    let definition = StackDefinition::new(
        policy,
        ComposeSource::new("services: {}\n"),
        ComposeSummary::default(),
    );
    let tombstone = Tombstone::of(directory(), &Stack::new(identity, definition)).unwrap();
    assert_eq!(tombstone.node().unwrap().as_str(), "files");
    assert_eq!(tombstone.keeping_node().node(), None);
}
