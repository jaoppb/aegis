//! Behaviour of the validation rules through their ports, with in-memory fakes.

use std::collections::BTreeMap;

use aegis_application::{
    DirEntry, EncryptionCheck, EntryKind, EntryName, Outcome, Reason, RepoSource, SourceError,
    validate_desired_state,
};
use aegis_domain::{
    Access, ComposeSource, ComposeSummary, ContainerPath, DesiredState, EnvFile, EnvFiles, Host,
    HostName, Hosts, Mount, Mounts, Presence, Problem, Rejection, Rejections, RepoPath, Service,
    ServiceName, ServiceSource, Services, Stack, StackDefinition, StackIdentity, StackKind,
    StackName, StackPolicy, Stacks, Volumes,
};

/// A repository of files; a file is "encrypted" when its text starts with `ENC`.
struct Memory(BTreeMap<String, String>);

impl Memory {
    fn with(files: &[(&str, &str)]) -> Self {
        let files = files
            .iter()
            .map(|(path, text)| ((*path).to_owned(), (*text).to_owned()));
        Self(files.collect())
    }
}

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

struct Prefix;

impl EncryptionCheck for Prefix {
    fn is_encrypted(&self, _file: &RepoPath, text: &str) -> bool {
        text.starts_with("ENC")
    }
}

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

fn outcome_of(service: Service) -> Option<Outcome> {
    let services = Services::new(vec![service]);
    let compose = ComposeSummary::new(services, Volumes::default());
    let source = ComposeSource::new("a\nb\n");
    let definition = StackDefinition::new(StackPolicy::default(), source, compose);
    let identity = StackIdentity::new(StackName::new("stack").ok()?, StackKind::Host);
    let stack = Stack::new(identity, definition);
    let host = Host::new(HostName::new("host").ok()?, Stacks::new(vec![stack]));
    let state = DesiredState::new(Hosts::new(vec![host]), Stacks::default());
    Some(Outcome::new(state, Rejections::default()))
}

fn relative(path: &str, access: Access) -> Option<Mount> {
    Some(Mount::Relative {
        source: RepoPath::new(path).ok()?,
        target: ContainerPath::new("/x").ok()?,
        access,
    })
}

fn env(path: &str, presence: Presence) -> Option<EnvFile> {
    Some(EnvFile::new(RepoPath::new(path).ok()?, presence))
}

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
