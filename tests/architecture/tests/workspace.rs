//! The Aegis workspace itself passes every check.

#[cfg(test)]
mod cases {
    use std::path::Path;

    #[test]
    fn the_workspace_is_clean() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let report = aegis_architecture_tests::check(&root).unwrap();
        assert!(report.is_clean(), "architecture violations:\n{report}");
    }
}
