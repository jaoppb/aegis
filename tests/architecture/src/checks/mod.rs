//! The rule families. Each exposes `check`, which returns its violations in a workspace.

pub mod exceptions;
pub mod fields;
pub mod manifests;
pub mod names;
pub mod size;
pub mod tuples;

use crate::violation::Report;
use crate::workspace::Workspace;

/// Runs every rule family on `workspace`.
#[must_use]
pub fn run_all(workspace: &Workspace) -> Report {
    let mut violations = fields::check(workspace);
    violations.extend(size::check(workspace));
    violations.extend(tuples::check(workspace));
    violations.extend(names::check(workspace));
    violations.extend(exceptions::check(workspace));
    violations.extend(manifests::check(workspace));
    Report::new(violations)
}
