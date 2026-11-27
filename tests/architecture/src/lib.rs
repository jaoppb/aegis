//! Architecture checks for the Aegis workspace.
//!
//! arch-lint enforces the layering (`arch-lint.toml`). The checks here cover what arch-lint cannot
//! express: struct shape, public fields, file size, tuple groupings, bag names, lint exceptions
//! and manifest rules. [`check`] reports every violation found under a workspace root.

pub mod checks;
pub mod error;
pub mod violation;
pub mod workspace;

use std::path::Path;

pub use error::CheckError;
pub use violation::{Report, Rule, Violation};

/// Runs every check on the workspace rooted at `root`.
///
/// # Errors
///
/// Returns [`CheckError`] when a file cannot be read or parsed.
pub fn check(root: &Path) -> Result<Report, CheckError> {
    let workspace = workspace::Workspace::open(root)?;
    Ok(checks::run_all(&workspace))
}
