//! Reads a repository checkout from the local filesystem.

use std::path::PathBuf;

use aegis_application::{DirEntry, EntryKind, RepoSource, SourceError};
use aegis_domain::RepoPath;

/// A [`RepoSource`] over a directory on disk.
#[derive(Debug, Clone)]
pub struct FsRepo {
    root: PathBuf,
}

impl FsRepo {
    /// Read the repository checked out at `root`.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn resolve(&self, path: &RepoPath) -> PathBuf {
        self.root.join(path.as_str())
    }
}

fn failure(path: &RepoPath) -> impl Fn(std::io::Error) -> SourceError {
    let path = path.to_string();
    move |error| SourceError {
        path: path.clone(),
        message: error.to_string(),
    }
}

fn entry(item: std::io::Result<std::fs::DirEntry>) -> std::io::Result<DirEntry> {
    let item = item?;
    let name = item.file_name().to_string_lossy().into_owned();
    let kind = if item.path().is_dir() {
        EntryKind::Directory
    } else {
        EntryKind::Other
    };
    Ok(DirEntry { name, kind })
}

impl RepoSource for FsRepo {
    fn contains(&self, path: &RepoPath) -> Result<bool, SourceError> {
        self.resolve(path).try_exists().map_err(failure(path))
    }

    fn list_dir(&self, path: &RepoPath) -> Result<Vec<DirEntry>, SourceError> {
        let listing = std::fs::read_dir(self.resolve(path)).map_err(failure(path))?;
        listing
            .map(entry)
            .collect::<std::io::Result<_>>()
            .map_err(failure(path))
    }

    fn read_file(&self, path: &RepoPath) -> Result<String, SourceError> {
        std::fs::read_to_string(self.resolve(path)).map_err(failure(path))
    }
}
