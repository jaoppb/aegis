//! Reads a repository checkout from the local filesystem.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

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
    let kind = if item.file_type()?.is_dir() {
        EntryKind::Directory
    } else {
        EntryKind::Other
    };
    Ok(DirEntry { name, kind })
}

/// The largest file the loader reads. Compose and policy files are small; the cap keeps a
/// committed oversized file from exhausting memory.
const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Read a regular file without following a symlink, so a committed link cannot pull in a file from
/// outside the checkout, and refuse anything over [`MAX_FILE_BYTES`].
fn read_regular_file(path: &Path) -> std::io::Result<String> {
    if !std::fs::symlink_metadata(path)?.is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    let mut text = String::new();
    File::open(path)?
        .take(MAX_FILE_BYTES.saturating_add(1))
        .read_to_string(&mut text)?;
    if u64::try_from(text.len()).map_or(true, |len| len > MAX_FILE_BYTES) {
        return Err(std::io::Error::other("file is too large"));
    }
    Ok(text)
}

/// Refuse a symlink in place of a directory, so a committed `hosts` or `aegis` link cannot walk
/// the loader out of the checkout.
fn require_directory(path: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(path)?.is_dir() {
        return Ok(());
    }
    Err(std::io::Error::other("not a directory"))
}

impl RepoSource for FsRepo {
    fn contains(&self, path: &RepoPath) -> Result<bool, SourceError> {
        self.resolve(path).try_exists().map_err(failure(path))
    }

    fn list_dir(&self, path: &RepoPath) -> Result<Vec<DirEntry>, SourceError> {
        let dir = self.resolve(path);
        require_directory(&dir).map_err(failure(path))?;
        let listing = std::fs::read_dir(dir).map_err(failure(path))?;
        listing
            .map(entry)
            .collect::<std::io::Result<_>>()
            .map_err(failure(path))
    }

    fn read_file(&self, path: &RepoPath) -> Result<String, SourceError> {
        read_regular_file(&self.resolve(path)).map_err(failure(path))
    }
}
