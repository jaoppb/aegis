//! The part of a compose file the validation rules reason about.

use crate::value::{ContainerPath, HostPath, RepoPath, ServiceName, VolumeName};

/// Whether a mount lets the container write to its source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// The container may only read.
    ReadOnly,
    /// The container may read and write.
    ReadWrite,
}

/// One mount of a service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mount {
    /// A named volume.
    Named {
        /// The volume name.
        volume: VolumeName,
        /// Where it is mounted in the container.
        target: ContainerPath,
        /// Read or write access.
        access: Access,
    },
    /// An absolute path on the host.
    HostPath {
        /// The host path.
        source: HostPath,
        /// Where it is mounted in the container.
        target: ContainerPath,
        /// Read or write access.
        access: Access,
    },
    /// A path relative to the stack directory, so it must exist in the repository.
    Relative {
        /// The path relative to the stack directory.
        source: RepoPath,
        /// Where it is mounted in the container.
        target: ContainerPath,
        /// Read or write access.
        access: Access,
    },
    /// An anonymous volume, created by Compose with no stable name.
    Anonymous {
        /// Where it is mounted in the container.
        target: ContainerPath,
    },
    /// An in-memory filesystem.
    Tmpfs {
        /// Where it is mounted in the container.
        target: ContainerPath,
    },
}

/// A service image built from the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    /// The build context, relative to the stack directory.
    pub context: RepoPath,
    /// The Dockerfile, relative to the context, when not the default.
    pub dockerfile: Option<RepoPath>,
}

/// One compose service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    /// The service name.
    pub name: ServiceName,
    /// The image reference, when the service declares one.
    pub image: Option<String>,
    /// The build, when the service is built from the repository.
    pub build: Option<Build>,
    /// The `env_file` entries, relative to the stack directory.
    pub env_files: Vec<RepoPath>,
    /// The mounts.
    pub mounts: Vec<Mount>,
}

/// The services and top-level volumes of a compose file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComposeSummary {
    /// The services, in name order.
    pub services: Vec<Service>,
    /// The top-level named volumes.
    pub volumes: Vec<VolumeName>,
}
