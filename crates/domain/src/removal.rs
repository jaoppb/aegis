//! What is left behind when a stack leaves the repository, and the one value that allows deleting
//! data: the [`Tombstone`] of a stack whose `aegis.toml` says `state = "purged"`.

use std::fmt;

use crate::compose::{Access, Mount};
use crate::deployment::{Detail, ProjectName};
use crate::policy::StackState;
use crate::state::{Stack, StackKind};
use crate::value::{HostPath, NodeLabel, RepoPath, ValueError, collection};

collection!(
    /// Absolute paths on the host.
    HostPaths,
    HostPath
);

collection!(
    /// What a removed stack left behind, one line each, such as `volume aegis-web_data`.
    Remains,
    Detail
);

collection!(
    /// Compose projects, by name.
    ProjectNames,
    ProjectName
);

/// A compose project Aegis started whose stack the repository no longer declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leftover {
    project: ProjectName,
    remains: Remains,
}

impl Leftover {
    /// The `project` and what it `remains` with.
    #[must_use]
    pub const fn new(project: ProjectName, remains: Remains) -> Self {
        Self { project, remains }
    }

    /// The compose project.
    #[must_use]
    pub const fn project(&self) -> &ProjectName {
        &self.project
    }

    /// The data it keeps.
    #[must_use]
    pub const fn remains(&self) -> &Remains {
        &self.remains
    }
}

collection!(
    /// The leftovers found on a host.
    Leftovers,
    Leftover
);

/// Why a stack cannot be purged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TombstoneError {
    /// The stack's `aegis.toml` does not say `state = "purged"`.
    NotPurged,
    /// Aegis's own stacks are never purged.
    Reserved,
    /// The stack has the same project name as a reserved stack, so purging it would delete that
    /// stack's data.
    SharesReservedProject,
    /// The reserved `aegis/` directory cannot be read, so which names are reserved is unknown.
    ReservedUnknown,
    /// The stack has no Compose project name.
    NoProject(ValueError),
}

impl fmt::Display for TombstoneError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotPurged => formatter.write_str("the stack is not marked purged"),
            Self::Reserved => formatter.write_str("a reserved stack cannot be purged"),
            Self::SharesReservedProject => formatter.write_str(
                "the stack has the project name of a reserved stack; rename it to purge it",
            ),
            Self::ReservedUnknown => formatter.write_str(
                "the aegis/ directory cannot be read, so reserved names are unknown; fix it and the next reconcile retries",
            ),
            Self::NoProject(error) => write!(formatter, "the stack has no project name: {error}"),
        }
    }
}

impl std::error::Error for TombstoneError {}

/// The order to delete a stack's data. It exists only for a user stack that declares
/// `state = "purged"`: every operation that deletes a volume, a path or a node takes one, so
/// nothing else can reach them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tombstone {
    directory: RepoPath,
    project: ProjectName,
    stack: Stack,
    node: Option<NodeLabel>,
}

impl Tombstone {
    /// The tombstone of `stack`, which lives in `directory`.
    ///
    /// # Errors
    ///
    /// Returns a [`TombstoneError`] unless `stack` is a user stack marked purged with a valid
    /// project name.
    pub fn of(directory: RepoPath, stack: &Stack) -> Result<Self, TombstoneError> {
        allow_purge(stack)?;
        let project = ProjectName::of(stack.name()).map_err(TombstoneError::NoProject)?;
        let node = exposed_node(stack);
        Ok(Self {
            directory,
            project,
            stack: stack.clone(),
            node,
        })
    }

    /// The same tombstone that leaves the tailnet node alone, because another stack now exposes
    /// a node with the same hostname.
    #[must_use]
    pub fn keeping_node(self) -> Self {
        Self { node: None, ..self }
    }

    /// The stack directory.
    #[must_use]
    pub const fn directory(&self) -> &RepoPath {
        &self.directory
    }

    /// The compose project whose volumes are deleted.
    #[must_use]
    pub const fn project(&self) -> &ProjectName {
        &self.project
    }

    /// The tailnet node to remove: the exposed service's hostname, when the stack exposes one.
    #[must_use]
    pub fn node(&self) -> Option<NodeLabel> {
        self.node.clone()
    }

    /// The absolute host paths the stack mounts read-write, once each.
    #[must_use]
    pub fn paths(&self) -> HostPaths {
        HostPaths::new(writable_paths(&self.stack).into_iter().collect())
    }
}

/// The hostname of the node a stack's sidecar enrols as: the name of the exposed service, or of
/// the service that joins the node.
#[must_use]
pub fn exposed_node(stack: &Stack) -> Option<NodeLabel> {
    let service = stack.policy().tailnet_service()?;
    NodeLabel::new(service.as_str()).ok()
}

fn allow_purge(stack: &Stack) -> Result<(), TombstoneError> {
    if stack.kind() == StackKind::Reserved {
        return Err(TombstoneError::Reserved);
    }
    if stack.policy().state() != StackState::Purged {
        return Err(TombstoneError::NotPurged);
    }
    Ok(())
}

fn host_mounts(stack: &Stack) -> impl Iterator<Item = &Mount> {
    let services = stack.compose().services().iter();
    services
        .flat_map(|service| service.mounts().iter())
        .filter(|mount| matches!(mount, Mount::HostPath { .. }))
}

fn writable_paths(stack: &Stack) -> std::collections::BTreeSet<HostPath> {
    host_mounts(stack).filter_map(writable_source).collect()
}

fn writable_source(mount: &Mount) -> Option<HostPath> {
    match mount {
        Mount::HostPath {
            source,
            access: Access::ReadWrite,
            ..
        } => Some(source.clone()),
        _ => None,
    }
}

/// Every absolute host path `stack` mounts, read-only or not.
#[must_use]
pub fn mounted_paths(stack: &Stack) -> HostPaths {
    let sources = host_mounts(stack).filter_map(|mount| match mount {
        Mount::HostPath { source, .. } => Some(source.clone()),
        _ => None,
    });
    HostPaths::new(sources.collect())
}
