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

/// The name of a directory entry, without its parent.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct EntryName(String);

impl EntryName {
    /// Wrap `name`.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// The name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether the name starts with a dot.
    #[must_use]
    pub fn is_hidden(&self) -> bool {
        self.0.starts_with('.')
    }
}

/// One entry of a directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    name: EntryName,
    kind: EntryKind,
}

impl DirEntry {
    /// An entry called `name` of `kind`.
    #[must_use]
    pub const fn new(name: EntryName, kind: EntryKind) -> Self {
        Self { name, kind }
    }

    /// The file or directory name, without its parent.
    #[must_use]
    pub const fn name(&self) -> &EntryName {
        &self.name
    }

    /// What the entry is.
    #[must_use]
    pub const fn kind(&self) -> EntryKind {
        self.kind
    }

    /// Whether the entry is a directory whose name does not start with a dot.
    #[must_use]
    pub fn is_visible_directory(&self) -> bool {
        self.kind == EntryKind::Directory && !self.name.is_hidden()
    }
}

/// Why a repository read failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reason(String);

impl Reason {
    /// The reason described by `cause`.
    #[must_use]
    pub fn new(cause: &impl fmt::Display) -> Self {
        Self(cause.to_string())
    }
}

impl fmt::Display for Reason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A repository read failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceError {
    path: RepoPath,
    reason: Reason,
}

impl SourceError {
    /// The read of `path` failed for `reason`.
    #[must_use]
    pub const fn new(path: RepoPath, reason: Reason) -> Self {
        Self { path, reason }
    }

    /// The path that could not be read.
    #[must_use]
    pub const fn path(&self) -> &RepoPath {
        &self.path
    }

    /// What went wrong.
    #[must_use]
    pub const fn reason(&self) -> &Reason {
        &self.reason
    }
}

impl fmt::Display for SourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.path, self.reason)
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
