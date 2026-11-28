//! Behaviour of the validation rules through their ports, with in-memory fakes.

use std::collections::BTreeMap;

use aegis_application::{
    DirEntry, EncryptionCheck, EntryKind, EntryName, Outcome, Reason, RepoSource, SourceError,
    validate_desired_state,
};
use aegis_domain::{
    Access, Build, ComposeSource, ComposeSummary, ContainerPath, DeclaredPath, DesiredState,
    EnvFile, EnvFiles, Expose, Host, HostName, HostPath, Hosts, Mount, Mounts, Port, Presence,
    Problem, Rejection, Rejections, RepoPath, Service, ServiceName, ServiceSource, Services, Stack,
    StackDefinition, StackIdentity, StackKind, StackName, StackPolicy, StackState, Stacks,
    TailnetTags, Violations, VolumeName, Volumes,
};

/// A repository of files; a file is "encrypted" when its text starts with `ENC`.
#[cfg(test)]
struct Memory(BTreeMap<String, String>);

#[cfg(test)]
impl Memory {
    fn with(files: &[(&str, &str)]) -> Self {
        let files = files
            .iter()
            .map(|(path, text)| ((*path).to_owned(), (*text).to_owned()));
        Self(files.collect())
    }
}

#[cfg(test)]
impl RepoSource for Memory {
    fn contains(&self, path: &RepoPath) -> Result<bool, SourceError> {
        Ok(self.0.contains_key(path.as_str()))
    }

    fn list_dir(&self, _path: &RepoPath) -> Result<Vec<DirEntry>, SourceError> {
        Ok(vec![DirEntry::new(EntryName::new("x"), EntryKind::Other)])
    }

    fn read_file(&self, path: &RepoPath) -> Result<String, SourceError> {
        let missing = || SourceError::new(path.clone(), Reason::new(&"missing"));
        self.0.get(path.as_str()).cloned().ok_or_else(missing)
    }
}

#[cfg(test)]
struct Prefix;

#[cfg(test)]
impl EncryptionCheck for Prefix {
    fn is_encrypted(&self, _file: &RepoPath, text: &str) -> bool {
        text.starts_with("ENC")
    }
}

#[cfg(test)]
fn service(mounts: Vec<Option<Mount>>, env_files: Vec<Option<EnvFile>>) -> Option<Service> {
    let name = ServiceName::new("app").ok()?;
    let mounts: Option<Vec<Mount>> = mounts.into_iter().collect();
    let env_files: Option<Vec<EnvFile>> = env_files.into_iter().collect();
    Some(
        Service::new(name, ServiceSource::default())
            .with_mounts(Mounts::new(mounts?))
            .with_env_files(EnvFiles::new(env_files?)),
    )
}

#[cfg(test)]
fn outcome_of(service: Service) -> Option<Outcome> {
    outcome_with(service, Volumes::default())
}

#[cfg(test)]
fn outcome_with(service: Service, volumes: Volumes) -> Option<Outcome> {
    let services = Services::new(vec![service]);
    let compose = ComposeSummary::new(services, volumes);
    let source = ComposeSource::new("a\nb\n");
    let definition = StackDefinition::new(StackPolicy::default(), source, compose);
    let identity = StackIdentity::new(StackName::new("stack").ok()?, StackKind::Host);
    let stack = Stack::new(identity, definition);
    let host = Host::new(HostName::new("host").ok()?, Stacks::new(vec![stack]));
    let state = DesiredState::new(Hosts::new(vec![host]), Stacks::default());
    Some(Outcome::new(state, Rejections::default()))
}

#[cfg(test)]
fn relative(path: &str, access: Access) -> Option<Mount> {
    Some(Mount::Relative {
        source: RepoPath::new(path).ok()?,
        target: ContainerPath::new("/x").ok()?,
        access,
    })
}

#[cfg(test)]
fn env(path: &str, presence: Presence) -> Option<EnvFile> {
    Some(EnvFile::new(RepoPath::new(path).ok()?, presence))
}

#[cfg(test)]
fn validated(
    files: &[(&str, &str)],
    mounts: Vec<Option<Mount>>,
    env_files: Vec<Option<EnvFile>>,
) -> Option<Outcome> {
    let outcome = outcome_of(service(mounts, env_files)?)?;
    Some(validate_desired_state(
        &outcome,
        &Memory::with(files),
        &Prefix,
    ))
}

#[cfg(test)]
fn problems(
    files: &[(&str, &str)],
    mounts: Vec<Option<Mount>>,
    env_files: Vec<Option<EnvFile>>,
) -> Option<Vec<Problem>> {
    let outcome = validated(files, mounts, env_files)?;
    let found = outcome.rejected().iter().flat_map(Rejection::violations);
    Some(found.map(|violation| violation.problem().clone()).collect())
}

#[test]
fn a_read_only_mount_of_an_existing_file_passes() {
    let files = [("hosts/host/stack/conf.toml", "x")];
    let mounts = vec![relative("./conf.toml", Access::ReadOnly)];
    assert!(problems(&files, mounts, vec![]).unwrap().is_empty());
}

#[test]
fn a_writable_relative_mount_is_a_violation() {
    let files = [("hosts/host/stack/data", "x")];
    let mounts = vec![relative("./data", Access::ReadWrite)];
    let found = problems(&files, mounts, vec![]).unwrap();
    assert!(matches!(
        found.as_slice(),
        [Problem::WritableRelativeMount { .. }]
    ));
}

#[test]
fn a_missing_and_writable_mount_reports_both() {
    let mounts = vec![relative("./data", Access::ReadWrite)];
    assert_eq!(problems(&[], mounts, vec![]).unwrap().len(), 2);
}

#[test]
fn a_plaintext_env_file_is_a_violation_and_an_encrypted_one_is_not() {
    let plain = [("hosts/host/stack/.env", "A=b")];
    let sealed = [("hosts/host/stack/.env", "ENC[a]")];
    let found = problems(&plain, vec![], vec![env(".env", Presence::Required)]).unwrap();
    assert!(matches!(found.as_slice(), [Problem::NotEncrypted { .. }]));
    let files = vec![env(".env", Presence::Required)];
    assert!(problems(&sealed, vec![], files).unwrap().is_empty());
}

#[test]
fn an_optional_env_file_may_be_absent_but_a_required_one_may_not() {
    let optional = vec![env(".env", Presence::Optional)];
    let required = vec![env(".env", Presence::Required)];
    assert!(problems(&[], vec![], optional).unwrap().is_empty());
    assert!(matches!(
        problems(&[], vec![], required).unwrap().as_slice(),
        [Problem::MissingFile { .. }]
    ));
}

#[test]
fn a_violation_points_at_the_compose_line() {
    let outcome = validated(&[], vec![], vec![env("b", Presence::Required)]).unwrap();
    let violation = &outcome.rejected().as_slice()[0].violations().as_slice()[0];
    assert_eq!(
        violation.to_string().split(": ").next(),
        Some("hosts/host/stack/compose.yaml:2")
    );
}

#[test]
fn a_rejected_stack_leaves_the_state() {
    let outcome = validated(&[], vec![], vec![env("b", Presence::Required)]).unwrap();
    assert!(outcome.state().hosts().as_slice()[0].stacks().is_empty());
    assert_eq!(outcome.rejected().len(), 1);
}

#[cfg(test)]
fn built(context: &str, dockerfile: Option<&str>) -> Option<Service> {
    let build = Build::new(
        DeclaredPath::new(context).ok()?,
        dockerfile.and_then(|file| DeclaredPath::new(file).ok()),
    );
    let name = ServiceName::new("app").ok()?;
    Some(Service::new(name, ServiceSource::new(None, Some(build))))
}

#[cfg(test)]
fn build_problems(files: &[(&str, &str)], service: Option<Service>) -> Option<Vec<Problem>> {
    let outcome = validate_desired_state(&outcome_of(service?)?, &Memory::with(files), &Prefix);
    let found = outcome.rejected().iter().flat_map(Rejection::violations);
    Some(found.map(|violation| violation.problem().clone()).collect())
}

#[test]
fn a_build_context_that_escapes_the_stack_is_a_violation() {
    for context in ["../shared", "/opt/src"] {
        let found = build_problems(&[], built(context, None)).unwrap();
        assert!(matches!(
            found.as_slice(),
            [Problem::BuildOutsideStack { .. }]
        ));
    }
}

#[test]
fn a_missing_build_context_is_a_violation() {
    let found = build_problems(&[], built("./app", None)).unwrap();
    assert!(matches!(
        found.as_slice(),
        [Problem::MissingBuildContext { .. }]
    ));
}

#[test]
fn a_dockerfile_that_leaves_the_stack_is_a_violation() {
    let files = [("hosts/host/stack/app", "x")];
    let found = build_problems(&files, built("./app", Some("../../Dockerfile"))).unwrap();
    assert!(matches!(
        found.as_slice(),
        [Problem::BuildOutsideStack { .. }]
    ));
    let inside = build_problems(&files, built("./app", Some("../Dockerfile"))).unwrap();
    assert!(inside.is_empty());
}

#[cfg(test)]
fn named_mount(volume: &str) -> Option<Mount> {
    Some(Mount::Named {
        volume: VolumeName::new(volume).ok()?,
        target: ContainerPath::new("/x").ok()?,
        access: Access::ReadWrite,
    })
}

#[cfg(test)]
fn mount_service(mounts: Vec<Option<Mount>>) -> Option<Service> {
    service(mounts, vec![])
}

#[cfg(test)]
fn volume_problems(declared: &[&str]) -> Option<Vec<Problem>> {
    let volumes: Option<Vec<VolumeName>> = declared
        .iter()
        .map(|name| VolumeName::new(*name).ok())
        .collect();
    let service = mount_service(vec![named_mount("data")])?;
    let outcome = outcome_with(service, Volumes::new(volumes?))?;
    let outcome = validate_desired_state(&outcome, &Memory::with(&[]), &Prefix);
    let found = outcome.rejected().iter().flat_map(Rejection::violations);
    Some(found.map(|violation| violation.problem().clone()).collect())
}

#[test]
fn a_named_volume_must_be_declared() {
    let found = volume_problems(&[]).unwrap();
    assert!(matches!(
        found.as_slice(),
        [Problem::UndeclaredVolume { .. }]
    ));
    assert!(volume_problems(&["data"]).unwrap().is_empty());
}

#[test]
fn host_path_anonymous_and_tmpfs_mounts_are_allowed() {
    let mounts = vec![
        Some(Mount::HostPath {
            source: HostPath::new("/srv/data").unwrap(),
            target: ContainerPath::new("/x").unwrap(),
            access: Access::ReadWrite,
        }),
        Some(Mount::Anonymous {
            target: ContainerPath::new("/a").unwrap(),
        }),
        Some(Mount::Tmpfs {
            target: ContainerPath::new("/t").unwrap(),
        }),
    ];
    assert!(problems(&[], mounts, vec![]).unwrap().is_empty());
}

#[test]
fn earlier_rejections_survive_and_the_valid_stack_is_kept() {
    let earlier = Rejection::new(
        RepoPath::new("hosts/host/broken").unwrap(),
        Violations::default(),
    );
    let outcome = outcome_of(service(vec![], vec![]).unwrap()).unwrap();
    let outcome = Outcome::new(
        outcome.state().clone(),
        Rejections::new(vec![earlier.clone()]),
    );
    let validated = validate_desired_state(&outcome, &Memory::with(&[]), &Prefix);
    assert_eq!(validated.rejected().as_slice(), [earlier]);
    assert_eq!(validated.state().hosts().as_slice()[0].stacks().len(), 1);
}

#[test]
fn a_failing_repository_read_is_a_violation() {
    let outcome = outcome_of(service(vec![relative("./x", Access::ReadOnly)], vec![]).unwrap());
    let validated = validate_desired_state(&outcome.unwrap(), &Broken, &Prefix);
    let found = validated.rejected().iter().flat_map(Rejection::violations);
    assert!(
        found
            .into_iter()
            .any(|violation| matches!(violation.problem(), Problem::Unreadable { .. }))
    );
}

#[cfg(test)]
struct Broken;

#[cfg(test)]
impl RepoSource for Broken {
    fn contains(&self, path: &RepoPath) -> Result<bool, SourceError> {
        Err(SourceError::new(path.clone(), Reason::new(&"boom")))
    }

    fn list_dir(&self, path: &RepoPath) -> Result<Vec<DirEntry>, SourceError> {
        Err(SourceError::new(path.clone(), Reason::new(&"boom")))
    }

    fn read_file(&self, path: &RepoPath) -> Result<String, SourceError> {
        Err(SourceError::new(path.clone(), Reason::new(&"boom")))
    }
}

#[test]
fn an_escaping_context_and_dockerfile_report_one_violation() {
    let found = build_problems(&[], built("../../ctx", Some("../../Dockerfile"))).unwrap();
    assert!(matches!(
        found.as_slice(),
        [Problem::BuildOutsideStack { .. }]
    ));
}

#[cfg(test)]
fn exposing(service_names: &[&str], exposed: &str) -> Vec<Problem> {
    exposing_with(service_names, exposed, &[])
}

#[cfg(test)]
fn exposing_with(service_names: &[&str], exposed: &str, volumes: &[&str]) -> Vec<Problem> {
    let names = service_names
        .iter()
        .map(|name| Service::new(ServiceName::new(*name).unwrap(), ServiceSource::default()));
    let declared = volumes.iter().map(|name| VolumeName::new(*name).unwrap());
    let compose = ComposeSummary::new(
        Services::new(names.collect()),
        Volumes::new(declared.collect()),
    );
    let expose = Expose::new(ServiceName::new(exposed).unwrap(), Port::new(80).unwrap());
    let policy = StackPolicy::new(Some(expose), TailnetTags::default(), StackState::Active);
    let definition = StackDefinition::new(policy, ComposeSource::new("a\nb\n"), compose);
    let identity = StackIdentity::new(StackName::new("stack").unwrap(), StackKind::Host);
    let stack = Stack::new(identity, definition);
    let host = Host::new(HostName::new("host").unwrap(), Stacks::new(vec![stack]));
    let state = DesiredState::new(Hosts::new(vec![host]), Stacks::default());
    let outcome = Outcome::new(state, Rejections::default());
    let validated = validate_desired_state(&outcome, &Memory::with(&[]), &Prefix);
    let found = validated.rejected().iter().flat_map(Rejection::violations);
    found.map(|violation| violation.problem().clone()).collect()
}

#[cfg(test)]
fn joining(services: Vec<Service>, joined: &str) -> Vec<Problem> {
    let compose = ComposeSummary::new(Services::new(services), Volumes::default());
    let node = aegis_domain::Node::new(ServiceName::new(joined).unwrap());
    let policy = StackPolicy::default().with_node(node);
    let definition = StackDefinition::new(policy, ComposeSource::new("a\nb\n"), compose);
    let identity = StackIdentity::new(StackName::new("stack").unwrap(), StackKind::Host);
    let host = Host::new(
        HostName::new("host").unwrap(),
        Stacks::new(vec![Stack::new(identity, definition)]),
    );
    let state = DesiredState::new(Hosts::new(vec![host]), Stacks::default());
    let outcome = Outcome::new(state, Rejections::default());
    let validated = validate_desired_state(&outcome, &Memory::with(&[]), &Prefix);
    let found = validated.rejected().iter().flat_map(Rejection::violations);
    found.map(|violation| violation.problem().clone()).collect()
}

#[cfg(test)]
fn service_with(name: &str, networking: aegis_domain::Networking) -> Service {
    Service::new(ServiceName::new(name).unwrap(), ServiceSource::default())
        .with_networking(networking)
}

#[test]
fn a_node_service_without_a_network_of_its_own_passes() {
    let services = vec![service_with("nfs", aegis_domain::Networking::Default)];
    assert!(joining(services, "nfs").is_empty());
}

#[test]
fn a_node_service_the_compose_file_lacks_is_a_violation() {
    let services = vec![service_with("web", aegis_domain::Networking::Default)];
    let found = joining(services, "nfs");
    assert!(matches!(
        found.as_slice(),
        [Problem::UnknownExposedService { .. }]
    ));
}

#[test]
fn a_node_service_with_its_own_network_is_a_violation() {
    let services = vec![service_with("nfs", aegis_domain::Networking::Own)];
    let found = joining(services, "nfs");
    assert!(matches!(
        found.as_slice(),
        [Problem::OwnNetworkOnNodeService { .. }]
    ));
}

#[test]
fn a_service_beside_the_node_service_may_set_its_own_network() {
    let services = vec![
        service_with("nfs", aegis_domain::Networking::Default),
        service_with("other", aegis_domain::Networking::Own),
    ];
    assert!(joining(services, "nfs").is_empty());
}

#[test]
fn a_node_stack_may_not_define_tailscale_or_caddy() {
    let services = vec![
        service_with("nfs", aegis_domain::Networking::Default),
        service_with("tailscale", aegis_domain::Networking::Default),
    ];
    let found = joining(services, "nfs");
    assert!(matches!(
        found.as_slice(),
        [Problem::ReservedSidecarService { .. }]
    ));
}

#[test]
fn an_exposed_service_with_its_own_network_is_not_a_violation() {
    let own = service_with("web", aegis_domain::Networking::Own);
    let compose = ComposeSummary::new(Services::new(vec![own]), Volumes::default());
    let expose = Expose::new(ServiceName::new("web").unwrap(), Port::new(80).unwrap());
    let policy = StackPolicy::new(Some(expose), TailnetTags::default(), StackState::Active);
    let definition = StackDefinition::new(policy, ComposeSource::new("a\nb\n"), compose);
    let identity = StackIdentity::new(StackName::new("stack").unwrap(), StackKind::Host);
    let host = Host::new(
        HostName::new("host").unwrap(),
        Stacks::new(vec![Stack::new(identity, definition)]),
    );
    let state = DesiredState::new(Hosts::new(vec![host]), Stacks::default());
    let outcome = Outcome::new(state, Rejections::default());
    let validated = validate_desired_state(&outcome, &Memory::with(&[]), &Prefix);
    assert!(validated.rejected().is_empty());
}

#[test]
fn an_exposed_service_the_compose_file_defines_passes() {
    assert!(exposing(&["web"], "web").is_empty());
}

#[test]
fn an_exposed_service_the_compose_file_lacks_is_a_violation() {
    let found = exposing(&["web"], "api");
    assert!(matches!(
        found.as_slice(),
        [Problem::UnknownExposedService { .. }]
    ));
}

#[test]
fn a_stack_that_exposes_a_service_may_not_define_tailscale_or_caddy() {
    let found = exposing(&["web", "tailscale", "caddy"], "web");
    assert_eq!(found.len(), 2);
    assert!(
        found
            .iter()
            .all(|problem| matches!(problem, Problem::ReservedSidecarService { .. }))
    );
}

#[test]
fn a_stack_without_exposure_may_define_a_service_called_caddy() {
    let outcome = outcome_of(Service::new(
        ServiceName::new("caddy").unwrap(),
        ServiceSource::default(),
    ))
    .unwrap();
    let validated = validate_desired_state(&outcome, &Memory::with(&[]), &Prefix);
    assert!(validated.rejected().is_empty());
}

#[test]
fn a_stack_that_exposes_a_service_may_not_declare_the_sidecar_volumes() {
    let found = exposing_with(
        &["web"],
        "web",
        &["tailscale-state", "tailscale-socket", "data"],
    );
    assert_eq!(found.len(), 2);
    assert!(
        found
            .iter()
            .all(|problem| matches!(problem, Problem::ReservedSidecarVolume { .. }))
    );
}
