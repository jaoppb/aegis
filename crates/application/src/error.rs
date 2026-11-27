//! Everything that can go wrong while loading a repository. The loader collects them all.

use std::fmt;

use aegis_domain::ValueError;

use crate::source::SourceError;

/// One problem found while loading the desired state.
///
/// Every variant carries the repository path, which names the host and the stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    /// A repository read failed.
    Source(SourceError),
    /// A directory name is not a valid host or stack name.
    InvalidName {
        /// The offending directory.
        path: String,
        /// Why the name was rejected.
        reason: ValueError,
    },
    /// A stack directory has no `compose.yaml`.
    MissingCompose {
        /// The `compose.yaml` that should exist.
        file: String,
    },
    /// A `compose.yaml` could not be understood.
    InvalidCompose {
        /// The `compose.yaml`.
        file: String,
        /// What was wrong.
        reason: String,
    },
    /// An `aegis.toml` is malformed or holds an invalid value.
    InvalidPolicy {
        /// The `aegis.toml`.
        file: String,
        /// What was wrong.
        reason: String,
    },
    /// An `aegis.toml` has a key outside the schema.
    UnknownKey {
        /// The `aegis.toml`.
        file: String,
        /// The unknown key.
        key: String,
    },
}

fn detail(formatter: &mut fmt::Formatter<'_>, file: &str, reason: &str) -> fmt::Result {
    write!(formatter, "{file}: {reason}")
}

impl fmt::Display for LoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(error) => error.fmt(formatter),
            Self::InvalidName { path, reason } => write!(formatter, "{path}: {reason}"),
            Self::MissingCompose { file } => write!(formatter, "{file}: missing"),
            Self::InvalidCompose { file, reason } => detail(formatter, file, reason),
            Self::InvalidPolicy { file, reason } => write!(formatter, "{file}: {reason}"),
            Self::UnknownKey { file, key } => write!(formatter, "{file}: unknown key `{key}`"),
        }
    }
}

impl std::error::Error for LoadError {}
