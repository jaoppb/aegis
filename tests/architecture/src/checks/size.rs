//! Source files have at most 1000 lines.

use crate::violation::{Location, Rule, Violation};
use crate::workspace::{SourceFile, Workspace};

/// The maximum number of lines in a source file.
pub const MAX_LINES: usize = 1000;

/// Rejects every Rust file, tests included, longer than [`MAX_LINES`].
#[must_use]
pub fn check(workspace: &Workspace) -> Vec<Violation> {
    workspace.sources().iter().filter_map(oversized).collect()
}

fn oversized(source: &SourceFile) -> Option<Violation> {
    let lines = source.text().lines().count();
    if lines <= MAX_LINES {
        return None;
    }
    let location = Location::new(source.path(), MAX_LINES.saturating_add(1));
    let message = format!("file has {lines} lines (max {MAX_LINES})");
    Some(Violation::new(Rule::FileSize, location, message))
}
