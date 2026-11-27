//! Loads the fixture repositories from disk through the real adapters.

use std::path::PathBuf;

use aegis_adapters::{FsRepo, SerdeStackParser};
use aegis_application::{LoadError, load_desired_state};
use aegis_domain::{
    Access, DesiredState, ImageRef, Mount, RepoPath, Service, Stack, StackKind, StackState,
    UpdatePolicy,
};

fn load(name: &str) -> Result<DesiredState, Vec<LoadError>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name);
    load_desired_state(&FsRepo::new(root), &SerdeStackParser)
}

fn stack(name: &str) -> Option<Stack> {
    let host = load("jao-server").ok()?.hosts().iter().next()?.clone();
    host.stacks()
        .iter()
        .find(|stack| stack.name().as_str() == name)
        .cloned()
}

/// The named service of the named stack.
fn service(stack_name: &str, service_name: &str) -> Option<Service> {
    let stack = stack(stack_name)?;
    let mut services = stack.compose().services().iter();
    services
        .find(|service| service.name().as_str() == service_name)
        .cloned()
}

#[test]
fn the_jao_server_stacks_load_into_typed_state() {
    let state = load("jao-server").unwrap();
    let host = &state.hosts().as_slice()[0];
    let names: Vec<&str> = host
        .stacks()
        .iter()
        .map(|stack| stack.name().as_str())
        .collect();
    let expected = [
        "ai-memory",
        "clip-cascade",
        "ferrous",
        "monitoring",
        "pihole",
        "proxy",
        "sharefile",
        "vaultwarden",
    ];
    assert_eq!(host.name().as_str(), "jao-server");
    assert_eq!(names, expected);
}

#[test]
fn a_named_volume_and_an_env_file_are_summarised() {
    let stack = stack("vaultwarden").unwrap();
    let service = &stack.compose().services().as_slice()[0];
    assert_eq!(
        service.source().image().map(ImageRef::as_str),
        Some("vaultwarden/server:1.34.3")
    );
    assert_eq!(service.env_files().as_slice()[0].as_str(), ".env");
    assert!(
        matches!(&service.mounts().as_slice()[0], Mount::Named { volume, .. } if volume.as_str() == "vaultwarden-data")
    );
    assert_eq!(
        stack.compose().volumes().as_slice()[0].as_str(),
        "vaultwarden-data"
    );
}

#[test]
fn a_relative_mount_is_read_only() {
    let prometheus = service("monitoring", "prometheus").unwrap();
    assert!(matches!(
        &prometheus.mounts().as_slice()[0],
        Mount::Relative {
            access: Access::ReadOnly,
            ..
        }
    ));
}

#[test]
fn a_host_path_mount_is_absolute() {
    let sharefile = service("sharefile", "sharefile").unwrap();
    assert!(matches!(
        &sharefile.mounts().as_slice()[0],
        Mount::HostPath {
            access: Access::ReadWrite,
            ..
        }
    ));
}

#[test]
fn a_long_form_build_is_summarised() {
    let build = service("ferrous", "ferrous")
        .unwrap()
        .source()
        .build()
        .cloned()
        .unwrap();
    assert_eq!(build.context().as_str(), ".");
    assert_eq!(build.dockerfile().unwrap().as_str(), "Dockerfile");
}

#[test]
fn a_short_form_build_is_the_context() {
    let build = service("sharefile", "sharefile")
        .unwrap()
        .source()
        .build()
        .cloned()
        .unwrap();
    assert_eq!(build.context().as_str(), "./app");
    assert_eq!(build.dockerfile(), None);
}

#[test]
fn the_long_mount_form_is_understood() {
    let caddy = service("proxy", "caddy").unwrap();
    assert!(
        matches!(&caddy.mounts().as_slice()[1], Mount::Named { volume, .. } if volume.as_str() == "caddy-data")
    );
    assert!(matches!(
        &caddy.mounts().as_slice()[2],
        Mount::HostPath {
            access: Access::ReadOnly,
            ..
        }
    ));
}

#[test]
fn env_file_lists_and_mappings_are_understood() {
    let grafana = service("monitoring", "grafana").unwrap();
    let paths: Vec<&str> = grafana.env_files().iter().map(RepoPath::as_str).collect();
    assert_eq!(paths, [".env", "./grafana.env"]);
}

#[test]
fn expose_and_tags_come_from_the_policy_file() {
    let policy = stack("vaultwarden").unwrap().policy().clone();
    let expose = policy.expose().unwrap();
    assert_eq!(
        (expose.service().as_str(), expose.port().get()),
        ("vaultwarden", 80)
    );
    assert_eq!(
        policy.tailnet_tags().as_slice()[0].as_str(),
        "tag:aegis-service"
    );
}

#[test]
fn a_stack_without_a_policy_file_has_the_defaults() {
    let policy = stack("ferrous").unwrap().policy().clone();
    assert_eq!(policy.expose(), None);
    assert_eq!(policy.update(), UpdatePolicy::Manual);
    assert_eq!(policy.state(), StackState::Active);
}

#[test]
fn aegis_own_stacks_are_reserved() {
    let reserved = load("jao-server").unwrap().reserved().clone();
    assert_eq!(reserved.as_slice()[0].name().as_str(), "controller");
    assert_eq!(reserved.as_slice()[0].kind(), StackKind::Reserved);
}

#[test]
fn every_problem_in_a_broken_repository_is_reported() {
    let errors = load("invalid").unwrap_err();
    let unknown = LoadError::UnknownKey {
        file: "hosts/jao-server/bad-policy/aegis.toml".to_owned(),
        key: "colour".to_owned(),
    };
    assert_eq!(errors.len(), 3);
    assert!(errors.contains(&unknown));
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, LoadError::MissingCompose { .. }))
    );
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, LoadError::InvalidCompose { .. }))
    );
}

#[test]
fn a_missing_repository_loads_as_empty() {
    assert_eq!(load("does-not-exist"), Ok(DesiredState::default()));
}
