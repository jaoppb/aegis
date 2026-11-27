//! Validates the fixture repositories from disk through the real adapters: each rule has a
//! passing and a failing stack.

use std::path::PathBuf;

use aegis_adapters::{FsRepo, SerdeStackParser, SopsCheck};
use aegis_application::{Outcome, load_desired_state, validate_desired_state};
use aegis_domain::Host;

fn validate(name: &str) -> Outcome {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name);
    let repo = FsRepo::new(root);
    let loaded = load_desired_state(&repo, &SerdeStackParser);
    validate_desired_state(&loaded, &repo, &SopsCheck)
}

fn kept(outcome: &Outcome) -> Vec<String> {
    let hosts = outcome.state().hosts().iter();
    let stacks = hosts.flat_map(Host::stacks);
    stacks.map(|stack| stack.name().to_string()).collect()
}

/// The rejected stack directories, with every violation rendered as `file:line: message`.
fn rejected(outcome: &Outcome) -> Vec<(String, Vec<String>)> {
    let rejected = outcome.rejected().iter();
    rejected
        .map(|rejection| {
            let found = rejection.violations().iter();
            let directory = rejection.directory().to_string();
            (directory, found.map(ToString::to_string).collect())
        })
        .collect()
}

fn violations_of(outcome: &Outcome, stack: &str) -> Vec<String> {
    let suffix = format!("/{stack}");
    let found = rejected(outcome)
        .into_iter()
        .find(|(directory, _)| directory.ends_with(&suffix));
    found.map(|(_, found)| found).unwrap_or_default()
}

#[test]
fn the_jao_server_fixture_is_valid() {
    let outcome = validate("jao-server");
    assert!(outcome.rejected().is_empty(), "{:?}", rejected(&outcome));
    assert_eq!(kept(&outcome).len(), 8);
}

#[test]
fn relative_mounts_must_be_read_only_and_exist() {
    let outcome = validate("validation/relative-mount");
    assert_eq!(kept(&outcome), ["pass"]);
    let writable = violations_of(&outcome, "writable");
    assert_eq!(writable.len(), 1);
    assert!(
        writable[0].starts_with("hosts/host/writable/compose.yaml:5: "),
        "{writable:?}"
    );
    assert!(writable[0].contains("must be `:ro`"));
    let missing = violations_of(&outcome, "missing");
    assert!(missing[0].contains("not in the repository"), "{missing:?}");
}

#[test]
fn named_volumes_must_be_declared() {
    let outcome = validate("validation/named-volume");
    assert_eq!(kept(&outcome), ["pass"]);
    let undeclared = violations_of(&outcome, "undeclared");
    assert!(undeclared[0].contains("not declared"), "{undeclared:?}");
}

#[test]
fn builds_stay_inside_the_stack_directory() {
    let outcome = validate("validation/build");
    assert_eq!(kept(&outcome), ["pass"]);
    assert_eq!(violations_of(&outcome, "outside").len(), 2);
    let dockerfile = violations_of(&outcome, "escaping-dockerfile");
    assert!(
        dockerfile[0].contains("outside the stack"),
        "{dockerfile:?}"
    );
    let missing = violations_of(&outcome, "missing");
    assert!(missing[0].contains("not in the repository"), "{missing:?}");
}

#[test]
fn env_and_secret_files_must_be_encrypted_and_present() {
    let outcome = validate("validation/secret-files");
    assert_eq!(kept(&outcome), ["pass"]);
    let plaintext = violations_of(&outcome, "plaintext");
    assert_eq!(plaintext.len(), 2, "{plaintext:?}");
    assert!(
        plaintext
            .iter()
            .all(|found| found.contains("not SOPS-encrypted"))
    );
    assert!(plaintext[0].contains("compose.yaml:"), "{plaintext:?}");
    let missing = violations_of(&outcome, "missing");
    assert!(
        missing[0].contains("env file `.env` is not in"),
        "{missing:?}"
    );
}

#[test]
fn included_extended_and_config_files_must_exist() {
    let outcome = validate("validation/secret-files");
    let found = violations_of(&outcome, "referenced");
    assert_eq!(found.len(), 3, "{found:?}");
    for name in ["shared.yaml", "base.yaml", "app.conf"] {
        assert!(
            found.iter().any(|line| line.contains(name)),
            "{name}: {found:?}"
        );
    }
}

#[test]
fn an_optional_env_file_that_is_present_must_still_be_encrypted() {
    let outcome = validate("validation/secret-files");
    let found = violations_of(&outcome, "optional-plaintext");
    assert!(found[0].contains("not SOPS-encrypted"), "{found:?}");
}

#[test]
fn a_missing_secret_file_is_a_violation() {
    let outcome = validate("validation/secret-files");
    let found = violations_of(&outcome, "missing-secret");
    assert!(
        found[0].contains("secret file `./secrets/db.yaml`"),
        "{found:?}"
    );
}

#[test]
fn the_hand_written_jao_server_layout_is_rejected_with_actionable_messages() {
    let outcome = validate("legacy-jao-server");
    assert!(kept(&outcome).is_empty());
    let vaultwarden = violations_of(&outcome, "vaultwarden");
    assert!(
        vaultwarden
            .iter()
            .any(|found| found.contains("must be `:ro`"))
    );
    assert!(
        vaultwarden
            .iter()
            .any(|found| found.contains("sops --encrypt"))
    );
    let pihole = violations_of(&outcome, "pihole");
    assert!(pihole.iter().any(|found| found.contains("etc-pihole")));
}

#[test]
fn an_invalid_stack_is_skipped_while_the_others_stay() {
    let outcome = validate("validation/relative-mount");
    assert_eq!(kept(&outcome), ["pass"]);
    assert_eq!(outcome.rejected().len(), 3);
    let missing = violations_of(&outcome, "missing");
    assert!(missing[0].contains("not in the repository"), "{missing:?}");
}

#[test]
fn a_long_form_bind_of_a_bare_name_is_a_relative_mount() {
    let outcome = validate("validation/relative-mount");
    let found = violations_of(&outcome, "long-bind");
    assert!(
        found.iter().any(|line| line.contains("must be `:ro`")),
        "{found:?}"
    );
}

#[test]
fn an_additional_build_context_stays_inside_the_stack() {
    let outcome = validate("validation/build");
    let found = violations_of(&outcome, "extra-context");
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("../shared"), "{found:?}");
}

#[test]
fn an_unreferenced_dotenv_must_be_encrypted() {
    let outcome = validate("validation/secret-files");
    let found = violations_of(&outcome, "implicit-env");
    assert!(found[0].contains("not SOPS-encrypted"), "{found:?}");
}

#[test]
fn included_and_extended_files_are_not_accepted_unvalidated() {
    let outcome = validate("validation/secret-files");
    let found = violations_of(&outcome, "referenced");
    assert!(
        found
            .iter()
            .filter(|line| line.contains("inline it"))
            .count()
            == 2,
        "{found:?}"
    );
}
