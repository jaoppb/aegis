//! The ports through which a removed stack is taken down and a tombstone is carried out, and the
//! rule that decides which absolute paths may be deleted.
//!
//! Deleting data is split from taking a stack down on purpose: [`Teardown`] and [`Projects`] can
//! only stop containers and read, while every method of [`Purger`] takes a
//! [`Tombstone`], which only a stack marked `state = "purged"` can produce.

use std::fmt;
use std::future::Future;

use aegis_domain::{HostPath, HostPaths, Leftovers, NodeLabel, ProjectName, Tombstone};

use crate::source::Reason;

/// Lists what Aegis once started on this host and that is still there.
pub trait Projects {
    /// The compose projects named `aegis-*` that run, or keep volumes, on this host and are
    /// scoped to it. It does not judge which of them the repository still declares.
    ///
    /// # Errors
    ///
    /// Returns a [`Reason`] when Docker cannot be asked.
    fn leftovers(&self) -> impl Future<Output = Result<Leftovers, Reason>> + Send;
}

/// Stops a compose project. It never deletes a volume or a path.
pub trait Teardown {
    /// `compose down` the `project`: remove its containers and networks, keep everything else.
    ///
    /// # Errors
    ///
    /// Returns a [`Reason`] when Compose cannot be run or reports a failure.
    fn down(&self, project: &ProjectName) -> impl Future<Output = Result<(), Reason>> + Send;
}

/// Removes a node from the tailnet, through the controller.
pub trait NodeDeleter {
    /// Delete the node `node`. A node that is already gone is a success.
    ///
    /// # Errors
    ///
    /// Returns a [`Reason`] when the controller cannot be reached, refuses, or finds the hostname
    /// ambiguous.
    fn delete_node(&self, node: &NodeLabel) -> impl Future<Output = Result<(), Reason>> + Send;
}

/// Deletes data. Every method takes the [`Tombstone`] that allows it.
pub trait Purger {
    /// Delete every named volume labelled as belonging to the tombstone's project, and no other.
    ///
    /// # Errors
    ///
    /// Returns a [`Reason`] when Docker cannot be run or refuses.
    fn remove_volumes(
        &self,
        tombstone: &Tombstone,
    ) -> impl Future<Output = Result<(), Reason>> + Send;

    /// Delete the directory at `path`, which passed the safety rule.
    ///
    /// # Errors
    ///
    /// Returns a [`Reason`] when the path cannot be deleted.
    fn remove_path(
        &self,
        tombstone: &Tombstone,
        path: &SafePath,
    ) -> impl Future<Output = Result<(), Reason>> + Send;
}

/// The fewest segments a path may have to be deleted: `/srv/data/web` qualifies, `/home/jao` does
/// not.
const MIN_DEPTH: usize = 3;

/// Why a path is not deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The path is too close to the root of the filesystem.
    TooShallow(HostPath),
    /// The path has a `.` or empty segment, or a character the delete cannot pass safely.
    Unusable(HostPath),
    /// The path is, holds or lies inside a path another stack or Aegis itself uses.
    Shared(HostPath),
    /// The path is a system location that no stack's data lives in.
    System(HostPath),
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShallow(path) => write!(formatter, "{path} is too close to the root"),
            Self::Unusable(path) => write!(formatter, "{path} is not a plain absolute path"),
            Self::Shared(path) => write!(formatter, "{path} is shared with another stack"),
            Self::System(path) => write!(formatter, "{path} is a system location"),
        }
    }
}

impl std::error::Error for Refusal {}

/// An absolute path that may be deleted: deep enough, plain, and not shared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafePath(HostPath);

/// The system locations a purge never deletes in or above: sockets, devices and the host's own
/// state are mounted read-write by many stacks.
const SYSTEM: [&str; 17] = [
    "/bin",
    "/boot",
    "/dev",
    "/etc",
    "/lib",
    "/proc",
    "/root",
    "/run",
    "/sbin",
    "/sys",
    "/usr",
    "/var/lib/docker",
    "/var/run",
    "/lib64",
    "/nix",
    "/var/lib/containerd",
    "/var/lib/containers",
];

/// The segments of `path` without empty and `.` segments, so `/srv/data/` and `/srv//data`
/// compare like `/srv/data`.
fn segments(path: &HostPath) -> Vec<&str> {
    let kept = path.as_str().split('/');
    kept.filter(|segment| !matches!(*segment, "" | "."))
        .collect()
}

fn is_system(path: &HostPath) -> bool {
    let candidates = SYSTEM.iter().filter_map(|text| HostPath::new(*text).ok());
    candidates.into_iter().any(|system| overlaps(path, &system))
}

fn is_plain(path: &HostPath) -> bool {
    let mut parts = path.as_str().split('/').skip(1);
    parts.all(|segment| !matches!(segment, "" | "."))
}

fn has_forbidden_char(path: &HostPath) -> bool {
    path.as_str().contains([':', ',', '\n', '\0'])
}

fn overlaps(first: &HostPath, second: &HostPath) -> bool {
    let (left, right) = (segments(first), segments(second));
    left.iter()
        .zip(right.iter())
        .all(|(one, other)| one == other)
}

fn verdict(path: &HostPath, taken: &HostPaths) -> Result<(), Refusal> {
    if !is_plain(path) || has_forbidden_char(path) {
        return Err(Refusal::Unusable(path.clone()));
    }
    if segments(path).len() < MIN_DEPTH {
        return Err(Refusal::TooShallow(path.clone()));
    }
    if is_system(path) {
        return Err(Refusal::System(path.clone()));
    }
    if taken.iter().any(|other| overlaps(path, other)) {
        return Err(Refusal::Shared(path.clone()));
    }
    Ok(())
}

impl SafePath {
    /// `path`, if it may be deleted while `taken` paths are in use.
    ///
    /// # Errors
    ///
    /// Returns a [`Refusal`] when the path is too shallow, not plain, or overlaps a taken path.
    pub fn check(path: &HostPath, taken: &HostPaths) -> Result<Self, Refusal> {
        verdict(path, taken)?;
        Ok(Self(path.clone()))
    }

    /// The path.
    #[must_use]
    pub const fn path(&self) -> &HostPath {
        &self.0
    }
}
