//! Aegis application layer: the ports the adapters implement and the repository loader built on
//! them. It does no IO.

pub mod error;
pub mod loader;
pub mod parse;
pub mod source;

pub use error::LoadError;
pub use loader::load_desired_state;
pub use parse::{ParseError, StackParser};
pub use source::{DirEntry, EntryKind, EntryName, Reason, RepoSource, SourceError};
