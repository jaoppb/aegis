//! The [`SecretStore`] that keeps decrypted files on tmpfs, and refuses any other filesystem so
//! plaintext never reaches persistent disk.
//!
//! Each project gets a private directory (`0700`) under the root, and each file is `0600`.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use aegis_application::{Reason, SecretStore, Slot, StoreError};
use aegis_domain::{HostPath, ProjectName, Secret};
use tokio::io::AsyncWriteExt;

/// The directory under `XDG_RUNTIME_DIR` (or `/dev/shm`) that holds every workspace.
const ROOT_NAME: &str = "aegis";

/// Where the kernel lists the mounts of this process.
const MOUNTINFO: &str = "/proc/self/mountinfo";

/// The mode bits of a directory (the file type bits).
const DIRECTORY: u32 = 0o04_0000;

/// Filesystem types that live in memory.
const VOLATILE: [&str; 2] = ["tmpfs", "ramfs"];

/// Keeps decrypted files under one root directory on tmpfs.
#[derive(Debug, Clone)]
pub struct TmpfsStore {
    root: PathBuf,
    mounts: PathBuf,
}

impl TmpfsStore {
    /// A store under `$XDG_RUNTIME_DIR/aegis`, or `/dev/shm/aegis` when it is unset.
    #[must_use]
    pub fn from_environment() -> Self {
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|value| !value.is_empty())
            .map_or_else(|| PathBuf::from("/dev/shm"), PathBuf::from);
        Self::at(base.join(ROOT_NAME))
    }

    /// A store under `root`. It still refuses to write unless `root` is on tmpfs.
    #[must_use]
    pub fn at<Root: Into<PathBuf>>(root: Root) -> Self {
        Self {
            root: root.into(),
            mounts: PathBuf::from(MOUNTINFO),
        }
    }

    /// The same store that reads the mount table from `mounts` instead of the kernel's, for tests.
    #[must_use]
    pub fn with_mount_table<Mounts: Into<PathBuf>>(self, mounts: Mounts) -> Self {
        Self {
            mounts: mounts.into(),
            ..self
        }
    }

    /// The root directory, created (private) and checked to be on tmpfs.
    async fn volatile_root(&self) -> Result<PathBuf, StoreError> {
        make_private_directory(&self.root).await?;
        let root = tokio::fs::canonicalize(&self.root)
            .await
            .map_err(|cause| io_failure("resolve the secret directory", &cause))?;
        let mounts = tokio::fs::read_to_string(&self.mounts)
            .await
            .map_err(|cause| io_failure("read the mount table", &cause))?;
        is_volatile(&mounts, &root)
            .then_some(root)
            .ok_or_else(|| failure("the secret directory is not on tmpfs"))
    }
}

fn failure(message: &str) -> StoreError {
    StoreError::new(Reason::new(&message))
}

fn io_failure(action: &str, cause: &std::io::Error) -> StoreError {
    StoreError::new(Reason::new(&format!("cannot {action}: {}", cause.kind())))
}

/// The user the agent runs as: the owner of its own `/proc` entry.
async fn own_uid() -> Result<u32, StoreError> {
    let own = tokio::fs::metadata("/proc/self").await;
    own.map(|metadata| metadata.uid())
        .map_err(|cause| io_failure("find the agent's user", &cause))
}

/// `path` as a real directory owned by the agent and closed to everyone else: created `0700` when
/// absent, refused when it is a link, belongs to someone else or lets others in. A shared
/// directory such as `/dev/shm` cannot hold a plaintext workspace another user prepared.
async fn make_private_directory(path: &Path) -> Result<(), StoreError> {
    let existing = tokio::fs::symlink_metadata(path).await;
    if matches!(&existing, Err(cause) if cause.kind() == std::io::ErrorKind::NotFound) {
        let created = tokio::fs::DirBuilder::new().mode(0o700).create(path).await;
        created.map_err(|cause| io_failure("create the secret directory", &cause))?;
    }
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|cause| io_failure("inspect the secret directory", &cause))?;
    let private = metadata.mode() & 0o17_0777 == DIRECTORY | 0o700;
    let owned = metadata.uid() == own_uid().await?;
    (private && owned)
        .then_some(())
        .ok_or_else(|| failure("the secret directory is not a private directory of the agent"))
}

/// Whether the mount that holds `path` is an in-memory filesystem, according to `mountinfo`: the
/// longest mount point that is a prefix of `path` decides.
fn is_volatile(mountinfo: &str, path: &Path) -> bool {
    let mounts = mountinfo.lines().filter_map(mount_of);
    let holding = mounts.filter(|mount| path.starts_with(&mount.point));
    holding
        .max_by_key(|mount| mount.point.components().count())
        .is_some_and(|mount| VOLATILE.contains(&mount.kind.as_str()))
}

/// One line of the mount table: where it is mounted and the filesystem type.
struct Mount {
    point: PathBuf,
    kind: String,
}

fn mount_of(line: &str) -> Option<Mount> {
    let (before, after) = line.split_once(" - ")?;
    let point = before.split_whitespace().nth(4)?;
    let kind = after.split_whitespace().next()?;
    Some(Mount {
        point: PathBuf::from(point.replace("\\040", " ")),
        kind: kind.to_owned(),
    })
}

async fn write_private(path: &Path, content: &Secret) -> Result<(), StoreError> {
    tokio::fs::remove_file(path).await.ok();
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .await
        .map_err(|cause| io_failure("create a secret file", &cause))?;
    file.write_all(content.expose().as_bytes())
        .await
        .map_err(|cause| io_failure("write a secret file", &cause))?;
    // A tokio file writes on a blocking thread: without a flush the bytes may not be on the file
    // yet when Compose reads it.
    file.flush()
        .await
        .map_err(|cause| io_failure("write a secret file", &cause))
}

async fn remove(path: &Path) -> Result<(), StoreError> {
    match tokio::fs::remove_dir_all(path).await {
        Err(cause) if cause.kind() != std::io::ErrorKind::NotFound => {
            Err(io_failure("remove the secret directory", &cause))
        }
        _ => Ok(()),
    }
}

impl SecretStore for TmpfsStore {
    async fn place(&self, slot: &Slot, content: &Secret) -> Result<HostPath, StoreError> {
        let root = self.volatile_root().await?;
        let directory = root.join(slot.project().as_str());
        make_private_directory(&directory).await?;
        let file = directory.join(slot.name().as_str());
        write_private(&file, content).await?;
        HostPath::new(file.to_string_lossy())
            .map_err(|_| failure("the secret directory is not a valid path"))
    }

    async fn discard(&self, project: &ProjectName) -> Result<(), StoreError> {
        remove(&self.root.join(project.as_str())).await
    }

    async fn sweep(&self) -> Result<(), StoreError> {
        remove(&self.root).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &str = "\
24 1 8:2 / / rw,relatime - ext4 /dev/sda2 rw
30 24 0:25 / /dev/shm rw,nosuid - tmpfs tmpfs rw
31 24 0:26 / /run/user/1000 rw - tmpfs tmpfs rw
32 31 0:40 / /run/user/1000/disk rw - ext4 /dev/sdb rw
";

    #[test]
    fn a_directory_on_tmpfs_is_volatile() {
        assert!(is_volatile(TABLE, Path::new("/dev/shm/aegis")));
        assert!(is_volatile(TABLE, Path::new("/run/user/1000/aegis")));
    }

    #[test]
    fn a_directory_on_disk_is_not_volatile() {
        assert!(!is_volatile(TABLE, Path::new("/home/jao/aegis")));
    }

    #[test]
    fn the_innermost_mount_decides() {
        assert!(!is_volatile(TABLE, Path::new("/run/user/1000/disk/aegis")));
    }
}
