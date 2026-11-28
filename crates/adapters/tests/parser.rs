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

fn rejection(text: &str) -> String {
    let error = compose::parse(text).err();
    error.map(|error| error.to_string()).unwrap_or_default()
}

#[test]
fn a_merge_key_inside_a_tagged_node_is_refused() {
    let text = "x-a: &a\n  image: alpine\nservices:\n  app: !override\n    <<: *a\n    volumes: ['./data:/data']\n";
    assert!(rejection(text).contains("`services.app`"));
}

#[test]
fn a_tagged_merge_key_is_refused() {
    let text = "x-a: &a\n  volumes: ['./data:/data']\nservices:\n  app:\n    image: x\n    !custom <<: *a\n";
    assert!(rejection(text).contains("`services.app.<<`"));
}

#[test]
fn every_yaml_tag_is_refused_with_its_path() {
    for tag in ["!reset", "!override", "!custom"] {
        let text = format!("services:\n  app:\n    image: x\n    ports: {tag} []\n");
        assert!(rejection(&text).contains("`services.app.ports`"), "{tag}");
    }
}

#[test]
fn a_tag_inside_an_anchor_is_refused() {
    let text = "x-a: &a !reset\n  image: x\nservices:\n  app:\n    <<: *a\n";
    assert!(rejection(text).contains("`x-a`"));
}

const REFUSED: &[(&str, &str)] = &[
    (
        "services:\n  app:\n    label_file: /etc/labels\n",
        "services.app.label_file",
    ),
    (
        "services:\n  app:\n    nonsense: 1\n",
        "services.app.nonsense",
    ),
    (
        "services:\n  app:\n    build: {context: ., ssh: [default]}\n",
        "services.app.build.ssh",
    ),
    (
        "services:\n  app:\n    build: {context: ., cache_to: ['type=local,dest=/x']}\n",
        "services.app.build.cache_to",
    ),
    (
        "services:\n  app:\n    volumes: [{type: bind, source: ./a, target: /a, flavour: x}]\n",
        "services.app.volumes[0].flavour",
    ),
    (
        "services:\n  app:\n    env_file: [{path: a.env, other: 1}]\n",
        "services.app.env_file[0].other",
    ),
    (
        "volumes:\n  v:\n    driver_opts: {}\n    mystery: 1\n",
        "volumes.v.mystery",
    ),
    (
        "secrets:\n  s:\n    file: a\n    mystery: 1\n",
        "secrets.s.mystery",
    ),
    ("mystery: 1\nservices: {}\n", "mystery"),
    (
        "include:\n  - path: a.yaml\n    env_file: .env\n",
        "include[0].env_file",
    ),
    (
        "services:\n  app:\n    provider: {type: x}\n",
        "services.app.provider",
    ),
    (
        "services:\n  app:\n    build: {context: ., cache_from: ['type=local,src=/x']}\n",
        "services.app.build.cache_from",
    ),
    (
        "services:\n  app:\n    secrets: [{source: s, bogus: 1}]\n",
        "services.app.secrets[0].bogus",
    ),
    (
        "services:\n  app:\n    configs: [{source: s, bogus: 1}]\n",
        "services.app.configs[0].bogus",
    ),
    (
        "services:\n  app:\n    build: {context: ., secrets: [{source: s, bogus: 1}]}\n",
        "services.app.build.secrets[0].bogus",
    ),
    (
        "services:\n  app:\n    networks: {n: {bogus: 1}}\n",
        "services.app.networks.n.bogus",
    ),
    ("networks:\n  n:\n    bogus: 1\n", "networks.n.bogus"),
    (
        "services:\n  app:\n    volumes: [{type: bind, source: ./a, target: /a, bind: {bogus: 1}}]\n",
        "services.app.volumes[0].bind.bogus",
    ),
    (
        "services:\n  app:\n    volumes: [{type: volume, source: v, target: /a, volume: {bogus: 1}}]\n",
        "services.app.volumes[0].volume.bogus",
    ),
    (
        "services:\n  app:\n    volumes: [{type: tmpfs, target: /a, tmpfs: {bogus: 1}}]\n",
        "services.app.volumes[0].tmpfs.bogus",
    ),
];

#[test]
fn unknown_and_host_path_keys_are_refused_at_every_level() {
    for (text, path) in REFUSED {
        assert!(rejection(text).contains(&format!("`{path}`")), "{path}");
    }
}

#[test]
fn every_unsupported_key_is_reported_at_once() {
    let text = "services:\n  app:\n    label_file: a\n    develop: {}\n";
    let message = rejection(text);
    assert!(message.contains("`services.app.label_file`"));
    assert!(message.contains("`services.app.develop`"));
}

#[test]
fn extension_keys_are_allowed_at_every_level() {
    let text = "x-top: 1\nservices:\n  app:\n    image: x\n    x-note: 1\n    build: {context: ., x-opt: 1}\n    volumes: [{type: volume, source: v, target: /v, x-m: 1}]\nvolumes:\n  v:\n    x-vol: 1\n";
    assert!(compose::parse(text).is_ok());
}

#[test]
fn plain_anchors_and_one_merge_level_are_still_accepted() {
    let text = "x-base: &base\n  restart: always\nservices:\n  app:\n    <<: *base\n    image: a\n";
    assert!(compose::parse(text).is_ok());
}

#[test]
fn interpolated_driver_options_are_refused() {
    for opts in [
        "{type: '$TYPE', o: bind, device: ./x}",
        "{type: none, o: 'bi$ND', device: ./x}",
        "{type: none, o: bind, device: '$DEVICE'}",
    ] {
        let text = format!("volumes:\n  v:\n    driver_opts: {opts}\n");
        assert!(rejection(&text).contains("interpolates"), "{opts}");
    }
}

#[test]
fn the_last_of_ro_and_rw_decides_the_access() {
    let read_write = first_mount("./data:/data:ro,rw").unwrap();
    assert!(matches!(
        read_write,
        Mount::Relative {
            access: Access::ReadWrite,
            ..
        }
    ));
    let read_only = first_mount("./data:/data:rw,ro").unwrap();
    assert!(matches!(
        read_only,
        Mount::Relative {
            access: Access::ReadOnly,
            ..
        }
    ));
}

#[test]
fn an_oci_layout_build_context_is_refused() {
    let text = "services:\n  app:\n    build:\n      context: .\n      additional_contexts:\n        base: oci-layout:///etc\n";
    assert!(rejection(text).contains("oci-layout"));
}

#[test]
fn devices_and_volumes_from_are_refused() {
    for key in ["devices: [/dev/x]", "volumes_from: ['container:x']"] {
        let text = format!("services:\n  app:\n    {key}\n");
        assert!(rejection(&text).contains("services.app."), "{key}");
    }
}

#[test]
fn a_chained_merge_names_the_left_over_key() {
    let text = "x-b: &b\n  volumes: ['./data:/data']\nx-a: &a\n  <<: *b\n  image: alpine\nservices:\n  app:\n    <<: *a\n";
    assert!(rejection(text).contains("`services.app.<<`"));
}

#[test]
fn valid_nested_compose_keys_are_accepted() {
    let text = "services:
  app:
    image: x
    secrets: [{source: s, target: t, uid: '1', gid: '1', mode: 288}]
    configs: [{source: c, target: /c}]
    networks:
      n: {aliases: [a], ipv4_address: 10.0.0.2, priority: 1}
    volumes:
      - {type: bind, source: ./a, target: /a, read_only: true, bind: {create_host_path: false}}
      - {type: volume, source: v, target: /v, volume: {nocopy: true}}
      - {type: tmpfs, target: /t, tmpfs: {size: 100}}
networks:
  n: {driver: bridge, internal: true, ipam: {config: [{subnet: 10.0.0.0/24}]}}
volumes:
  v: {driver: local, labels: {a: b}}
secrets:
  s: {file: ./s.env}
configs:
  c: {content: hi}
";
    assert!(compose::parse(text).is_ok());
}

#[test]
fn driver_options_without_a_dollar_are_accepted() {
    let text =
        "volumes:\n  v:\n    driver_opts: {type: nfs, o: 'addr=10.0.0.1,rw', device: ':/export'}\n";
    assert!(compose::parse(text).is_ok());
}

#[test]
fn a_git_at_context_without_a_host_and_path_is_a_checkout_path() {
    let found = extra_contexts("\n        a: git@x/../../..\n");
    assert_eq!(found.unwrap(), ["git@x/../../.."]);
}

#[test]
fn additional_contexts_with_other_schemes_are_refused() {
    for value in [
        "oci-layout:///etc",
        "https_x://y",
        "foo://bar",
        "HTTPS://x/y",
    ] {
        let text = format!(
            "services:\n  app:\n    build:\n      context: .\n      additional_contexts:\n        a: {value}\n"
        );
        assert!(
            rejection(&text).contains("scheme Aegis does not support"),
            "{value}"
        );
    }
}
