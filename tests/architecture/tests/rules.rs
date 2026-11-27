//! Each check fails on its failing fixture and passes on its passing one. Fixtures are `.txt` files
//! under `fixtures/rules`, so neither cargo nor the checker scans them as workspace code.

use std::path::Path;

use aegis_architecture_tests::Rule;
use aegis_architecture_tests::checks::{self, size};
use aegis_architecture_tests::workspace::{Manifest, SourceFile, Workspace};

#[cfg(test)]
mod cases {
    use super::*;

    const DOMAIN: &str = "crates/domain/src/example.rs";
    const ADAPTERS: &str = "crates/adapters/src/example.rs";

    fn rust(path: &str, text: &str) -> Workspace {
        let source = SourceFile::parse(Path::new(path), text.to_owned()).unwrap();
        Workspace::new(vec![source], Vec::new())
    }

    fn manifest(path: &str, text: &str) -> Workspace {
        let parsed = Manifest::parse(Path::new(path), text).unwrap();
        Workspace::new(Vec::new(), vec![parsed])
    }

    fn rules(workspace: &Workspace) -> Vec<Rule> {
        checks::run_all(workspace).rules()
    }

    fn fixture(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/rules")
            .join(name);
        std::fs::read_to_string(path).unwrap()
    }

    fn rules_in_domain(name: &str) -> Vec<Rule> {
        rules(&rust(DOMAIN, &fixture(name)))
    }

    fn assert_fails(name: &str, expected: Rule) {
        assert_eq!(rules_in_domain(name), vec![expected], "fixture {name}");
    }

    fn assert_passes(name: &str) {
        assert_eq!(rules_in_domain(name), Vec::new(), "fixture {name}");
    }

    #[test]
    fn struct_fields_allow_four_and_reject_five() {
        assert_fails("five_fields.rs.txt", Rule::StructFields);
        assert_passes("four_fields.rs.txt");
    }

    #[test]
    fn primitive_fields_are_rejected_beside_other_fields() {
        assert_fails("primitive_field.rs.txt", Rule::PrimitiveField);
        assert_fails("option_primitive_field.rs.txt", Rule::PrimitiveField);
        assert_passes("newtypes.rs.txt");
    }

    #[test]
    fn primitive_and_collection_rules_skip_other_layers() {
        let text = fixture("primitive_field.rs.txt");
        assert_eq!(rules(&rust(ADAPTERS, &text)), Vec::new());
        let text = fixture("collection_field.rs.txt");
        assert_eq!(rules(&rust(ADAPTERS, &text)), Vec::new());
    }

    #[test]
    fn collections_stand_alone() {
        assert_fails("collection_field.rs.txt", Rule::CollectionField);
        assert_passes("collection_newtype.rs.txt");
    }

    #[test]
    fn fields_are_private() {
        assert_fails("pub_field.rs.txt", Rule::PubField);
        assert_fails("pub_crate_field.rs.txt", Rule::PubField);
        assert_fails("pub_tuple_field.rs.txt", Rule::PubField);
        assert_passes("private_fields.rs.txt");
    }

    #[test]
    fn adapter_wire_structs_may_expose_fields() {
        let text = fixture("wire_struct.rs.txt");
        assert_eq!(rules(&rust(ADAPTERS, &text)), Vec::new());
        assert_eq!(rules(&rust(DOMAIN, &text)), vec![Rule::PubField]);
    }

    #[test]
    fn test_modules_are_exempt_from_the_shape_rules() {
        assert_passes("test_module_exempt.rs.txt");
    }

    #[test]
    fn integration_tests_are_exempt_from_the_shape_rules() {
        let text = fixture("pub_field.rs.txt");
        assert_eq!(
            rules(&rust("crates/domain/tests/example.rs", &text)),
            Vec::new()
        );
    }

    #[test]
    fn files_have_at_most_a_thousand_lines() {
        let limit = "fn f() {}\n".repeat(size::MAX_LINES);
        assert_eq!(rules(&rust(DOMAIN, &limit)), Vec::new());
        let over = format!("{limit}fn g() {{}}\n");
        assert_eq!(rules(&rust(DOMAIN, &over)), vec![Rule::FileSize]);
    }

    #[test]
    fn size_applies_to_tests_too() {
        let over = "fn f() {}\n".repeat(size::MAX_LINES.saturating_add(1));
        let workspace = rust("crates/domain/tests/example.rs", &over);
        assert_eq!(rules(&workspace), vec![Rule::FileSize]);
    }

    #[test]
    fn tuples_are_not_groupings() {
        assert_fails("tuple_param.rs.txt", Rule::TupleSignature);
        assert_fails("tuple_return.rs.txt", Rule::TupleSignature);
        assert_fails("tuple_pattern_param.rs.txt", Rule::TupleSignature);
        assert_fails("tuple_alias.rs.txt", Rule::TupleAlias);
        assert_fails("tuple_field.rs.txt", Rule::TupleField);
        assert_fails("tuple_positional_field.rs.txt", Rule::TupleField);
    }

    #[test]
    fn tuple_lookalikes_pass() {
        assert_passes("tuples_allowed.rs.txt");
    }

    #[test]
    fn bag_names_are_rejected() {
        assert_fails("bag_name.rs.txt", Rule::BagName);
        assert_fails("bag_name_suffix_dto.rs.txt", Rule::BagName);
        assert_passes("bag_name_ok.rs.txt");
    }

    #[test]
    fn lint_exceptions_are_rejected_in_every_form() {
        assert_fails("outer_allow.rs.txt", Rule::LintException);
        assert_fails("outer_expect.rs.txt", Rule::LintException);
        assert_fails("inner_allow.rs.txt", Rule::LintException);
        assert_fails("cfg_attr_allow.rs.txt", Rule::LintException);
        assert_fails("suppression_comment.rs.txt", Rule::SuppressionComment);
        assert_passes("clean_attributes.rs.txt");
    }

    #[test]
    fn lint_exceptions_are_rejected_in_tests_too() {
        let text = fixture("outer_allow.rs.txt");
        let workspace = rust("crates/domain/tests/example.rs", &text);
        assert_eq!(rules(&workspace), vec![Rule::LintException]);
    }

    #[test]
    fn crate_dependencies_come_from_the_workspace() {
        let path = "crates/example/Cargo.toml";
        let loose = manifest(path, &fixture("crate_dependency.toml.txt"));
        assert_eq!(rules(&loose), vec![Rule::ManifestDependency]);
        let targeted = manifest(path, &fixture("crate_target_dependency.toml.txt"));
        assert_eq!(rules(&targeted), vec![Rule::ManifestDependency]);
        let inherited = manifest(path, &fixture("crate_workspace_dependency.toml.txt"));
        assert_eq!(rules(&inherited), Vec::new());
    }

    #[test]
    fn workspace_dependencies_pin_exact_versions() {
        let loose = manifest("Cargo.toml", &fixture("root_loose_version.toml.txt"));
        assert_eq!(
            rules(&loose),
            vec![Rule::ManifestVersion, Rule::ManifestVersion]
        );
        let exact = manifest("Cargo.toml", &fixture("root_exact_versions.toml.txt"));
        assert_eq!(rules(&exact), Vec::new());
    }
}
