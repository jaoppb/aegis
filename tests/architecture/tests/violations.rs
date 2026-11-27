//! Proves the layering rules in `arch-lint.toml` actually fail on violations: the fixture
//! workspace under `fixtures/violations` breaks every rule once and each must be reported.

use arch_lint::Session;
use std::collections::BTreeSet;
use std::path::Path;

#[test]
fn fixture_violations_are_reported() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let result = Session::from_config_path(
        manifest.join("fixtures/violations"),
        manifest.join("../../arch-lint.toml"),
    )
    .and_then(Session::run)
    .expect("arch-lint runs on the fixture");
    let fired: BTreeSet<&str> = result
        .violations
        .iter()
        .map(|violation| violation.rule.as_str())
        .collect();
    let expected = BTreeSet::from([
        "domain-is-innermost",
        "domain-is-pure",
        "application-depends-on-domain-only",
        "application-has-no-io",
        "adapters-do-not-reach-binaries",
        "cli-is-independent",
        "controller-is-independent",
        "agent-is-independent",
    ]);
    assert_eq!(fired, expected);
}
