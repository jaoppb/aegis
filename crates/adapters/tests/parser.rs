//! The YAML and TOML parsers, on their own.

use aegis_adapters::{compose, policy};
use aegis_application::ParseError;
use aegis_domain::{Access, Mount, RepoPath, StackState, ValueError};

fn first_mount(mount: &str) -> Result<Mount, ParseError> {
    let text = format!("services:\n  app:\n    volumes:\n      - {mount}\n");
    let summary = compose::parse(&text)?;
    let service = summary.services().iter().next();
    let found = service.and_then(|service| service.mounts().iter().next().cloned());
    found.ok_or_else(|| ParseError::Invalid("no mount".to_owned()))
}

#[test]
fn an_empty_document_is_an_empty_compose_file() {
    assert_eq!(compose::parse("").unwrap().services().len(), 0);
}

#[test]
fn malformed_yaml_is_invalid() {
    assert!(matches!(
        compose::parse("services: [x"),
        Err(ParseError::InvalidAt { .. } | ParseError::Invalid(_))
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
    assert_eq!(compose::parse(text).unwrap().services().len(), 1);
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
        policy::parse("state = \"purged\"").unwrap().state(),
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
    assert!(matches!(
        policy::parse("= ="),
        Err(ParseError::InvalidAt { .. })
    ));
}

#[test]
fn malformed_toml_reports_its_line() {
    let Err(ParseError::InvalidAt { line, .. }) = policy::parse("state = \"active\"\n= =") else {
        panic!("expected a located error");
    };
    assert_eq!(line.to_string(), "2");
}

#[test]
fn file_references_are_collected() {
    let text = "include:\n  - ./a.yaml\n  - path: [./b.yaml, ./c.yaml]\nservices:\n  app:\n    extends:\n      file: ./base.yaml\n      service: base\n  other:\n    extends: app\nconfigs:\n  conf:\n    file: ./conf.toml\nsecrets:\n  key:\n    file: ./key.yaml\n  env:\n    environment: HOME\n";
    let summary = compose::parse(text).unwrap();
    let referenced: Vec<&str> = summary
        .referenced_files()
        .iter()
        .map(|file| file.path().as_str())
        .collect();
    assert_eq!(
        referenced,
        [
            "./conf.toml",
            "./a.yaml",
            "./b.yaml",
            "./c.yaml",
            "./base.yaml"
        ]
    );
    let secrets: Vec<&str> = summary
        .secret_files()
        .iter()
        .map(RepoPath::as_str)
        .collect();
    assert_eq!(secrets, ["./key.yaml"]);
}

#[test]
fn build_contexts_may_escape_the_stack_for_validation_to_report() {
    let text = "services:\n  app:\n    build:\n      context: ../shared\n      dockerfile: /abs/Dockerfile\n";
    let summary = compose::parse(text).unwrap();
    let build = summary.services().as_slice()[0].source().build().unwrap();
    assert!(build.context_inside_stack().is_none());
    assert!(build.dockerfile_escapes());
}

#[test]
fn remote_build_contexts_are_rejected() {
    for context in [
        "https://example.com/repo.git",
        "git@example.com:org/repo.git",
        "github.com/org/repo",
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
    for mount in ["/srv/${DIR}/x:/a", "./${DIR}:/a"] {
        let error = first_mount(mount).unwrap_err().to_string();
        assert!(
            error.contains(&ValueError::Interpolation.to_string()),
            "{mount}: {error}"
        );
    }
}

#[test]
fn a_bind_mount_needs_a_source() {
    let text = "services:\n  app:\n    volumes:\n      - type: bind\n        target: /a\n";
    assert!(
        matches!(compose::parse(text), Err(ParseError::Invalid(reason)) if reason.contains("needs a source"))
    );
}

#[test]
fn a_local_context_that_looks_like_a_host_is_still_a_path() {
    let text = "services:\n  app:\n    build: ./github.com/x\n";
    assert_eq!(
        compose::parse(text).unwrap().services().as_slice()[0]
            .source()
            .build()
            .unwrap()
            .context()
            .as_str(),
        "./github.com/x"
    );
}

#[test]
fn an_empty_image_is_rejected() {
    let text = "services:\n  app:\n    image: ''\n";
    assert!(matches!(
        compose::parse(text),
        Err(ParseError::Invalid(reason)) if reason.contains("value is empty")
    ));
}

fn extra_contexts(entries: &str) -> Option<Vec<String>> {
    let text = format!(
        "services:\n  app:\n    build:\n      context: ./app\n      additional_contexts:{entries}\n"
    );
    let summary = compose::parse(&text).ok()?;
    let service = summary.services().iter().next()?;
    let extra = service.source().build()?.additional_contexts().iter();
    Some(extra.map(|path| path.as_str().to_owned()).collect())
}

#[test]
fn additional_contexts_in_list_form_keep_their_paths() {
    let found = extra_contexts("\n        - shared=../shared\n        - ./bare\n");
    assert_eq!(found.unwrap(), ["../shared", "./bare"]);
}

#[test]
fn additional_contexts_that_are_not_paths_are_skipped() {
    let entries = "\n        a: service:web\n        b: git@host:repo.git\n        c: github.com/x/y\n        d: docker-image://alpine\n        e: ./sub\n";
    assert_eq!(extra_contexts(entries).unwrap(), ["./sub"]);
}

#[test]
fn home_directory_paths_are_rejected_everywhere_a_path_is_read() {
    let cases = [
        "services:\n  a:\n    env_file: ~/.env\n",
        "secrets:\n  k:\n    file: ~/.ssh/id_rsa\n",
        "configs:\n  k:\n    file: ~/c\n",
        "services:\n  a:\n    volumes:\n      - type: bind\n        source: ~/data\n        target: /d\n",
    ];
    for text in cases {
        assert!(compose::parse(text).is_err(), "{text}");
    }
}

#[test]
fn a_home_directory_build_context_escapes_the_stack() {
    let summary = compose::parse("services:\n  a:\n    build: ~/app\n").unwrap();
    let build = summary.services().as_slice()[0].source().build().unwrap();
    assert!(build.context_inside_stack().is_none());
}

#[test]
fn a_named_volume_may_not_bind_a_relative_or_home_device() {
    for device in ["./data", "../..", "~/x", "data", "2024"] {
        let text = format!(
            "volumes:\n  v:\n    driver_opts:\n      type: none\n      o: bind\n      device: {device}\n"
        );
        assert!(compose::parse(&text).is_err(), "{device}");
    }
}

#[test]
fn a_named_volume_may_bind_an_absolute_device_or_have_no_options() {
    let text = "volumes:\n  v:\n    driver_opts:\n      type: none\n      o: bind\n      device: /srv/data\n  w:\n  x: {}\n";
    assert_eq!(compose::parse(text).unwrap().volumes().len(), 3);
}

#[test]
fn a_device_that_is_not_a_bind_is_left_alone() {
    let nfs = "volumes:\n  v:\n    driver_opts:\n      type: nfs\n      o: addr=10.0.0.1\n      device: \":/export\"\n";
    assert_eq!(compose::parse(nfs).unwrap().volumes().len(), 1);
}

#[test]
fn merge_keys_are_applied_before_the_rules_see_the_file() {
    let text = "x-base: &base\n  volumes:\n    - ./data:/data\nservices:\n  app:\n    <<: *base\n    image: a\n";
    let summary = compose::parse(text).unwrap();
    let service = &summary.services().as_slice()[0];
    assert!(matches!(
        &service.mounts().as_slice()[0],
        Mount::Relative { .. }
    ));
    let volume = "x-vol: &v\n  driver_opts: {type: none, o: bind, device: ./data}\nvolumes:\n  d:\n    <<: *v\n";
    assert!(compose::parse(volume).is_err());
}

#[test]
fn chained_merge_keys_are_refused_rather_than_silently_dropped() {
    let service = "x-b: &b\n  volumes: [\"./data:/data\"]\nx-a: &a\n  <<: *b\n  image: alpine\nservices:\n  app:\n    <<: *a\n";
    assert!(compose::parse(service).is_err());
    let options = "x-d: &d {type: none, o: bind, device: ./x}\nx-b: &b\n  <<: *d\nvolumes:\n  data:\n    driver_opts:\n      <<: *b\n";
    assert!(compose::parse(options).is_err());
}

#[test]
fn a_bind_needs_only_one_of_type_none_or_o_bind_to_be_checked() {
    for opts in ["{o: bind, device: ./x}", "{type: none, device: ./x}"] {
        let text = format!("volumes:\n  v:\n    driver_opts: {opts}\n");
        assert!(compose::parse(&text).is_err(), "{opts}");
    }
    let bare = "volumes:\n  v:\n    driver_opts: {type: none, o: bind}\n";
    assert!(compose::parse(bare).is_ok());
}
