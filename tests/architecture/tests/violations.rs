//! Proves the layering rules in `arch-lint.toml` actually fail on violations: the fixture
//! workspace under `fixtures/violations` breaks every rule once and must be reported.

use arch_lint::Session;
use std::path::Path;

#[test]
fn fixture_violations_are_reported() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/violations");
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../arch-lint.toml");
    let result = Session::from_config_path(&root, &config)
        .and_then(Session::run)
        .expect("arch-lint runs on the fixture");
    let mut messages: Vec<String> = result
        .violations
        .iter()
        .map(|v| v.message.clone())
        .collect();
    messages.sort();
    let all = messages.join("\n");
    for expected in [
        "domain must not depend on any other layer",
        "domain must not do IO",
        "application may depend only on domain",
        "application declares ports",
        "adapters implement application ports",
        "binaries must not depend on each other",
    ] {
        assert!(
            all.contains(expected),
            "missing violation `{expected}` in:\n{all}"
        );
    }
}
