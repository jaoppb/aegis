//! The YAML and TOML parsers, on their own.

use aegis_adapters::{compose, policy};
use aegis_application::ParseError;
use aegis_domain::{Access, Mount, StackState, ValueError};

fn first_mount(mount: &str) -> Result<Mount, ParseError> {
    let text = format!("services:\n  app:\n    volumes:\n      - {mount}\n");
    let summary = compose::parse(&text)?;
    let service = summary.services.into_iter().next();
    let found = service.and_then(|service| service.mounts.into_iter().next());
    found.ok_or_else(|| ParseError::Invalid("no mount".to_owned()))
}

#[test]
fn an_empty_document_is_an_empty_compose_file() {
    assert_eq!(compose::parse("").unwrap().services.len(), 0);
}

#[test]
fn malformed_yaml_is_invalid() {
    assert!(matches!(
        compose::parse("services: [x"),
        Err(ParseError::Invalid(_))
    ));
}

#[test]
fn a_short_mount_of_an_absolute_source_is_a_host_path() {
    assert!(matches!(
        first_mount("/srv/a:/a").unwrap(),
        Mount::HostPath { .. }
    ));
}

#[test]
fn a_short_mount_of_a_dotted_source_is_relative() {
    let mount = first_mount("./a:/a:ro").unwrap();
    assert!(matches!(
        mount,
        Mount::Relative {
            access: Access::ReadOnly,
            ..
        }
    ));
}

#[test]
fn a_short_mount_of_a_bare_source_is_a_named_volume() {
    let mount = first_mount("data:/a:rw").unwrap();
    assert!(matches!(
        mount,
        Mount::Named {
            access: Access::ReadWrite,
            ..
        }
    ));
}

#[test]
fn a_short_mount_of_a_target_only_is_anonymous() {
    assert!(matches!(
        first_mount("/a").unwrap(),
        Mount::Anonymous { .. }
    ));
}

#[test]
fn short_mount_options_are_searched_for_ro() {
    let mount = first_mount("data:/a:z,ro").unwrap();
    assert!(matches!(
        mount,
        Mount::Named {
            access: Access::ReadOnly,
            ..
        }
    ));
}

#[test]
fn a_relative_mount_may_not_leave_the_stack() {
    assert!(first_mount("../secret:/a").is_err());
}

#[test]
fn a_mount_target_must_be_absolute() {
    let error = ValueError::NotAbsolute.to_string();
    assert!(
        first_mount("data:relative")
            .unwrap_err()
            .to_string()
            .contains(&error)
    );
}

#[test]
fn unsupported_mount_types_are_rejected() {
    let text = "services:\n  app:\n    volumes:\n      - type: npipe\n        target: /a\n";
    assert!(compose::parse(text).is_err());
}

#[test]
fn unrelated_compose_keys_are_ignored() {
    let text =
        "name: demo\nservices:\n  app:\n    image: x\n    ports: ['1:1']\n    healthcheck: {}\n";
    assert_eq!(compose::parse(text).unwrap().services.len(), 1);
}

#[test]
fn an_empty_policy_is_the_default() {
    assert_eq!(
        policy::parse("").unwrap(),
        aegis_domain::StackPolicy::default()
    );
}

#[test]
fn the_state_is_read() {
    assert_eq!(
        policy::parse("state = \"purged\"").unwrap().state,
        StackState::Purged
    );
}

#[test]
fn an_unknown_top_level_key_is_a_typed_error() {
    assert_eq!(
        policy::parse("colour = 1"),
        Err(ParseError::UnknownKey("colour".to_owned()))
    );
}

#[test]
fn an_unknown_key_under_expose_is_a_typed_error() {
    let text = "[expose]\nservice = \"web\"\nport = 80\nhost = \"x\"\n";
    assert_eq!(
        policy::parse(text),
        Err(ParseError::UnknownKey("expose.host".to_owned()))
    );
}

#[test]
fn invalid_values_are_not_unknown_keys() {
    for text in [
        "state = \"gone\"",
        "update = \"auto\"",
        "tailnet_tags = [\"nope\"]",
        "[expose]\nservice = \"web\"\nport = 0",
    ] {
        assert!(
            matches!(policy::parse(text), Err(ParseError::Invalid(_))),
            "{text}"
        );
    }
}

#[test]
fn malformed_toml_is_invalid() {
    assert!(matches!(policy::parse("= ="), Err(ParseError::Invalid(_))));
}

#[test]
fn remote_build_contexts_are_rejected() {
    for context in [
        "https://example.com/repo.git",
        "git@example.com:org/repo.git",
    ] {
        let text = format!("services:\n  app:\n    build: {context}\n");
        assert!(
            matches!(compose::parse(&text), Err(ParseError::Invalid(_))),
            "{context}"
        );
    }
}

#[test]
fn interpolated_paths_are_rejected() {
    assert!(first_mount("/srv/${DIR}/x:/a").is_err());
    assert!(first_mount("./${DIR}:/a").is_err());
}

#[test]
fn a_bind_mount_needs_a_source() {
    let text = "services:\n  app:\n    volumes:\n      - type: bind\n        target: /a\n";
    assert!(compose::parse(text).is_err());
}
