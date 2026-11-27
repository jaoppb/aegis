//! Struct names say what the thing is: no `Args`, `Params`, `Dto`, `Data`, `Info` or `Bag` ending.

use crate::violation::{Location, Rule, Violation};
use crate::workspace::{SourceFile, Workspace};

/// The struct name endings that are rejected.
pub const BAG_SUFFIXES: [&str; 6] = ["Args", "Params", "Dto", "Data", "Info", "Bag"];

/// Rejects structs named with a bag suffix in crate `src` code.
#[must_use]
pub fn check(workspace: &Workspace) -> Vec<Violation> {
    let production = workspace
        .sources()
        .iter()
        .filter(|source| source.is_production());
    production.flat_map(violations_in).collect()
}

fn violations_in(source: &SourceFile) -> Vec<Violation> {
    source
        .structs()
        .into_iter()
        .filter_map(|item| finding(source, &item.ident))
        .collect()
}

fn finding(source: &SourceFile, ident: &syn::Ident) -> Option<Violation> {
    let name = ident.to_string();
    let suffix = BAG_SUFFIXES
        .iter()
        .find(|suffix| name.ends_with(**suffix))?;
    let location = Location::new(source.path(), ident.span().start().line);
    let message = format!("struct {name} ends in {suffix}; name what the thing is");
    Some(Violation::new(Rule::BagName, location, message))
}
