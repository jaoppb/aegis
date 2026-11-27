//! The port through which the loader reads the repository checkout.

use std::fmt;

use aegis_domain::RepoPath;

/// What a directory entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// A directory.
    Directory,
    /// Anything else: a regular file, a symlink or a special file.
    Other,
}

/// One entry of a directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    /// The file or directory name, without its parent.
    pub name: String,
    /// What the entry is.
    pub kind: EntryKind,
}

/// A repository read failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceError {
    /// The path that could not be read.
    pub path: String,
    /// What went wrong.
    pub message: String,
}

impl fmt::Display for SourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.path, self.message)
    }
}

impl std::error::Error for SourceError {}

/// Read access to a checked-out repository. Paths are relative to the repository root.
pub trait RepoSource {
    /// Whether a file or directory exists at `path`.
    ///
    /// # Errors
    ///
    /// Returns a [`SourceError`] when the existence check itself fails.
    fn contains(&self, path: &RepoPath) -> Result<bool, SourceError>;

    /// The entries of the directory at `path`, in any order.
    ///
    /// # Errors
    ///
    /// Returns a [`SourceError`] when `path` is not a readable directory.
    fn list_dir(&self, path: &RepoPath) -> Result<Vec<DirEntry>, SourceError>;

    /// The UTF-8 contents of the file at `path`.
    ///
    /// # Errors
    ///
    /// Returns a [`SourceError`] when `path` is not a readable UTF-8 file.
    fn read_file(&self, path: &RepoPath) -> Result<String, SourceError>;
}
