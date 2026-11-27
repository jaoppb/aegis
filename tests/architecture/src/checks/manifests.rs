//! Manifest rules: crates declare dependencies only as `name.workspace = true`, and the root
//! `[workspace.dependencies]` pin each registry crate with an exact `=` version.

use toml::{Table, Value};

use crate::violation::{Location, Rule, Violation};
use crate::workspace::{Manifest, Workspace};

const DEPENDENCY_TABLES: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];

/// Applies the dependency rules to every manifest.
#[must_use]
pub fn check(workspace: &Workspace) -> Vec<Violation> {
    workspace
        .manifests()
        .iter()
        .flat_map(violations_in)
        .collect()
}

fn violations_in(manifest: &Manifest) -> Vec<Violation> {
    if manifest.is_root() {
        return pins(manifest);
    }
    let declared = dependency_entries(manifest.table());
    let loose = declared.iter().filter(|entry| !entry.is_workspace());
    loose
        .map(|entry| not_workspace(manifest, entry.name()))
        .collect()
}

/// One dependency declaration: its name and what it is set to.
struct Dependency<'toml> {
    name: &'toml str,
    value: &'toml Value,
}

impl Dependency<'_> {
    const fn name(&self) -> &str {
        self.name
    }

    fn is_workspace(&self) -> bool {
        let inherited = self.value.get("workspace");
        inherited.and_then(Value::as_bool) == Some(true)
    }

    /// The registry version requirement, absent for path dependencies.
    fn version(&self) -> Option<&str> {
        match self.value {
            Value::String(version) => Some(version),
            other => other.get("version").and_then(Value::as_str),
        }
    }
}

fn entries_of<'toml>(section: &'toml Table, kind: &str) -> Vec<Dependency<'toml>> {
    let table = section.get(kind).and_then(Value::as_table);
    let entries = table.into_iter().flat_map(Table::iter);
    entries
        .map(|(name, value)| Dependency {
            name: name.as_str(),
            value,
        })
        .collect()
}

fn dependency_entries(table: &Table) -> Vec<Dependency<'_>> {
    let targets = table.get("target").and_then(Value::as_table);
    let target_tables = targets
        .into_iter()
        .flat_map(Table::values)
        .filter_map(Value::as_table);
    let nested = target_tables.flat_map(tables_in);
    tables_in(table).into_iter().chain(nested).collect()
}

fn tables_in(table: &Table) -> Vec<Dependency<'_>> {
    DEPENDENCY_TABLES
        .iter()
        .flat_map(|kind| entries_of(table, kind))
        .collect()
}

fn pins(manifest: &Manifest) -> Vec<Violation> {
    let workspace = manifest.table().get("workspace").and_then(Value::as_table);
    let declared = workspace
        .into_iter()
        .flat_map(|section| entries_of(section, "dependencies"));
    let loose = declared.filter(|entry| {
        entry
            .version()
            .is_some_and(|version| !version.starts_with('='))
    });
    loose
        .map(|entry| not_exact(manifest, entry.name()))
        .collect()
}

fn not_workspace(manifest: &Manifest, name: &str) -> Violation {
    let message = format!("dependency {name} must be declared with `workspace = true`");
    let location = Location::new(manifest.path(), 1);
    Violation::new(Rule::ManifestDependency, location, message)
}

fn not_exact(manifest: &Manifest, name: &str) -> Violation {
    let message = format!("workspace dependency {name} must pin an exact `=` version");
    let location = Location::new(manifest.path(), 1);
    Violation::new(Rule::ManifestVersion, location, message)
}
