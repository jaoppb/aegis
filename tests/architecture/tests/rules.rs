//! Each check fails on its failing fixture and passes on its passing one. Fixtures are `.txt` files
//! under `fixtures/rules`, so neither cargo nor the checker scans them as workspace code.

use std::path::Path;

use aegis_architecture_tests::checks::{self, size};
use aegis_architecture_tests::violation::Location;
use aegis_architecture_tests::workspace::{Manifest, SourceFile, Workspace};
use aegis_architecture_tests::{Report, Rule, Violation};

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

    fn assert_clean_in(path: &str, name: &str) {
        assert_eq!(rules(&rust(path, &fixture(name))), Vec::new(), "{name}");
    }

    fn rules_in(path: &str, name: &str) -> Vec<Rule> {
        rules(&rust(path, &fixture(name)))
    }

    #[test]
    fn the_serde_exemption_is_limited_to_adapter_wire_structs() {
        let plain = rules_in(ADAPTERS, "adapter_plain_pub.rs.txt");
        assert_eq!(plain, vec![Rule::PubField]);
        assert_clean_in(ADAPTERS, "wire_serialize.rs.txt");
        assert_clean_in(ADAPTERS, "wire_qualified.rs.txt");
    }

    #[test]
    fn application_is_modelled_and_binaries_are_not() {
        const APPLICATION: &str = "crates/application/src/example.rs";
        const BINARY: &str = "crates/cli/src/example.rs";
        let primitive = rules_in(APPLICATION, "primitive_field.rs.txt");
        assert_eq!(primitive, vec![Rule::PrimitiveField]);
        let collection = rules_in(APPLICATION, "collection_field.rs.txt");
        assert_eq!(collection, vec![Rule::CollectionField]);
        assert_clean_in(BINARY, "primitive_field.rs.txt");
        assert_clean_in(BINARY, "collection_field.rs.txt");
    }

    #[test]
    fn every_tuple_wrapper_is_looked_through() {
        let workspace = rust(DOMAIN, &fixture("tuple_nested.rs.txt"));
        assert_eq!(rules(&workspace), vec![Rule::TupleSignature; 8]);
    }

    #[test]
    fn tuple_structs_count_their_fields() {
        assert_fails("tuple_struct_five.rs.txt", Rule::StructFields);
    }

    #[test]
    fn exception_edge_forms() {
        assert_fails("arch_lint_allow.rs.txt", Rule::LintException);
        assert_fails("suppression_no_space.rs.txt", Rule::SuppressionComment);
        assert_fails("suppression_indented.rs.txt", Rule::SuppressionComment);
        assert_passes("cfg_attr_condition_allow.rs.txt");
    }

    #[test]
    fn suppression_comments_are_found_wherever_they_sit() {
        assert_fails("suppression_trailing.rs.txt", Rule::SuppressionComment);
        assert_fails("suppression_doc.rs.txt", Rule::SuppressionComment);
        assert_fails("suppression_extra_space.rs.txt", Rule::SuppressionComment);
        assert_fails("macros_allow.rs.txt", Rule::LintException);
        assert_passes("comment_ok.rs.txt");
    }

    #[test]
    fn git_dependencies_need_a_rev() {
        let git = manifest("Cargo.toml", &fixture("root_git.toml.txt"));
        assert_eq!(rules(&git), vec![Rule::ManifestVersion]);
        let loose = manifest("Cargo.toml", &fixture("root_git_loose_version.toml.txt"));
        assert_eq!(rules(&loose), vec![Rule::ManifestVersion]);
    }

    #[test]
    fn only_test_modules_are_exempt() {
        assert_fails("feature_module_checked.rs.txt", Rule::PubField);
        assert_fails("cfg_not_test_module.rs.txt", Rule::PubField);
        assert_passes("test_module_bag_name.rs.txt");
    }

    #[test]
    fn dev_and_build_dependencies_come_from_the_workspace() {
        let path = "crates/example/Cargo.toml";
        let both = manifest(path, &fixture("crate_dev_build_dependencies.toml.txt"));
        assert_eq!(rules(&both), vec![Rule::ManifestDependency; 2]);
        let disabled = manifest(path, &fixture("crate_workspace_false.toml.txt"));
        assert_eq!(rules(&disabled), vec![Rule::ManifestDependency]);
    }

    #[test]
    fn table_form_versions_are_checked_and_paths_are_exempt() {
        let loose = manifest("Cargo.toml", &fixture("root_table_loose.toml.txt"));
        assert_eq!(rules(&loose), vec![Rule::ManifestVersion]);
        let exact = manifest("Cargo.toml", &fixture("root_table_exact.toml.txt"));
        assert_eq!(rules(&exact), Vec::new());
    }

    #[test]
    fn roles_follow_the_path() {
        let exposed = vec![Rule::PubField];
        let name = "pub_field.rs.txt";
        assert_eq!(rules_in("crates/domain/src/bin/tool.rs", name), exposed);
        assert_eq!(rules_in("tests/architecture/src/lib.rs", name), exposed);
        assert_clean_in("crates/domain/build.rs", name);
        assert_clean_in("crates/domain/examples/demo.rs", name);
    }

    #[test]
    fn reports_are_ordered_by_file_then_line_then_rule() {
        let place = |file, line| Location::new(Path::new(file), line);
        let report = Report::new(vec![
            Violation::new(Rule::TupleField, place("b.rs", 1), "b"),
            Violation::new(Rule::BagName, place("a.rs", 10), "ten"),
            Violation::new(Rule::TupleAlias, place("a.rs", 5), "five"),
            Violation::new(Rule::BagName, place("a.rs", 5), "five"),
        ]);
        let expected = vec![
            Rule::BagName,
            Rule::TupleAlias,
            Rule::BagName,
            Rule::TupleField,
        ];
        assert_eq!(report.rules(), expected);
    }
}
