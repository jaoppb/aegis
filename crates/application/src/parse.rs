//! The port that turns file contents into domain types, so wire formats stay in the adapters.

use std::fmt;

use aegis_domain::{ComposeSummary, Line, StackPolicy};

/// A file's contents could not be turned into domain types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The file has a key the schema does not know.
    UnknownKey(String),
    /// The file is malformed or holds an invalid value.
    Invalid(String),
    /// The file is malformed at a known line.
    InvalidAt {
        /// What was wrong.
        reason: String,
        /// Where.
        line: Line,
    },
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownKey(key) => write!(formatter, "unknown key `{key}`"),
            Self::Invalid(reason) | Self::InvalidAt { reason, .. } => formatter.write_str(reason),
        }
    }
}

impl std::error::Error for ParseError {}

/// Parses the two files of a stack directory.
pub trait StackParser {
    /// Summarise the services and volumes of a `compose.yaml`.
    ///
    /// # Errors
    ///
    /// Returns a [`ParseError`] when `text` is not a valid compose file.
    fn parse_compose(&self, text: &str) -> Result<ComposeSummary, ParseError>;

    /// Read the policy of an `aegis.toml`.
    ///
    /// # Errors
    ///
    /// Returns [`ParseError::UnknownKey`] for a key outside the schema and
    /// [`ParseError::Invalid`] for any other malformed content.
    fn parse_policy(&self, text: &str) -> Result<StackPolicy, ParseError>;
}
