//! Zero lint exceptions: no `allow` or `expect` attribute in any form (outer, inner or inside
//! `cfg_attr`), no `arch_lint::allow` and no arch-lint suppression comment. Tests are included.

use syn::punctuated::Punctuated;
use syn::visit::Visit;
use syn::{Attribute, Meta, Token};

use crate::violation::{Location, Rule, Violation};
use crate::workspace::{SourceFile, Workspace};

const EXCEPTION_ATTRIBUTES: [&str; 3] = ["allow", "expect", "arch_lint::allow"];
const SUPPRESSION_PREFIXES: [&str; 2] = ["// arch-lint:", "//arch-lint:"];

/// Rejects lint-exception attributes and arch-lint suppression comments in every Rust file.
#[must_use]
pub fn check(workspace: &Workspace) -> Vec<Violation> {
    let sources = workspace.sources().iter();
    sources
        .flat_map(|source| attributes_in(source).into_iter().chain(comments_in(source)))
        .collect()
}

#[derive(Default)]
struct AttributeCollector<'ast> {
    attributes: Vec<&'ast Attribute>,
}

impl<'ast> Visit<'ast> for AttributeCollector<'ast> {
    fn visit_attribute(&mut self, attribute: &'ast Attribute) {
        self.attributes.push(attribute);
    }
}

fn attributes_in(source: &SourceFile) -> Vec<Violation> {
    let mut collector = AttributeCollector::default();
    collector.visit_file(source.syntax());
    collector
        .attributes
        .into_iter()
        .flat_map(|attribute| attribute_violations(source, attribute))
        .collect()
}

fn attribute_violations(source: &SourceFile, attribute: &Attribute) -> Vec<Violation> {
    let line = attribute.pound_token.span.start().line;
    forbidden_names(&attribute.meta)
        .into_iter()
        .map(|name| {
            let message = format!("lint exception attribute #[{name}(...)] is forbidden");
            Violation::new(
                Rule::LintException,
                Location::new(source.path(), line),
                message,
            )
        })
        .collect()
}

fn path_name(meta: &Meta) -> String {
    let segments = meta.path().segments.iter();
    let names: Vec<String> = segments.map(|segment| segment.ident.to_string()).collect();
    names.join("::")
}

fn forbidden_names(meta: &Meta) -> Vec<String> {
    let name = path_name(meta);
    if EXCEPTION_ATTRIBUTES.contains(&name.as_str()) {
        return vec![name];
    }
    if name != "cfg_attr" {
        return Vec::new();
    }
    cfg_attr_metas(meta)
        .iter()
        .flat_map(forbidden_names)
        .collect()
}

/// The attributes a `cfg_attr(condition, a, b)` applies: everything after the condition.
fn cfg_attr_metas(meta: &Meta) -> Vec<Meta> {
    let Meta::List(list) = meta else {
        return Vec::new();
    };
    let parsed = list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated);
    parsed
        .map(|metas| metas.into_iter().skip(1).collect())
        .unwrap_or_default()
}

fn comments_in(source: &SourceFile) -> Vec<Violation> {
    let lines = (1_usize..).zip(source.text().lines());
    lines
        .filter(|&(_, line)| is_suppression(line))
        .map(|(number, _)| {
            let location = Location::new(source.path(), number);
            let message = "arch-lint suppression comments are forbidden";
            Violation::new(Rule::SuppressionComment, location, message)
        })
        .collect()
}

fn is_suppression(line: &str) -> bool {
    let trimmed = line.trim_start();
    SUPPRESSION_PREFIXES
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
}
