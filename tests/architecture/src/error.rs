//! The ways the checks can fail to run at all.

use std::path::PathBuf;

/// A workspace file could not be read or parsed, so the checks cannot vouch for it.
#[derive(Debug, thiserror::Error)]
pub enum CheckError {
    /// A file or directory could not be read.
    #[error("cannot read {path}: {source}")]
    Read {
        /// The path that failed.
        path: PathBuf,
        /// The IO failure.
        source: std::io::Error,
    },
    /// A Rust file is not valid syntax.
    #[error("cannot parse {path}: {source}")]
    Syntax {
        /// The path that failed.
        path: PathBuf,
        /// The parser's complaint.
        source: syn::Error,
    },
    /// A `Cargo.toml` is not valid TOML.
    #[error("cannot parse {path}: {source}")]
    Manifest {
        /// The path that failed.
        path: PathBuf,
        /// The parser's complaint.
        source: toml::de::Error,
    },
}
