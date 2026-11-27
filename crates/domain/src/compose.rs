//! The part of a compose file the validation rules reason about.

use crate::value::{
    ContainerPath, HostPath, ImageRef, RepoPath, ServiceName, VolumeName, collection,
};

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
    context: RepoPath,
    dockerfile: Option<RepoPath>,
}

impl Build {
    /// A build of `context`, with `dockerfile` when it is not the default.
    #[must_use]
    pub const fn new(context: RepoPath, dockerfile: Option<RepoPath>) -> Self {
        Self {
            context,
            dockerfile,
        }
    }

    /// The build context, relative to the stack directory.
    #[must_use]
    pub const fn context(&self) -> &RepoPath {
        &self.context
    }

    /// The Dockerfile, relative to the context, when not the default.
    #[must_use]
    pub const fn dockerfile(&self) -> Option<&RepoPath> {
        self.dockerfile.as_ref()
    }
}

/// Where a service gets its image: a reference, a build from the repository, or neither.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServiceSource {
    image: Option<ImageRef>,
    build: Option<Build>,
}

impl ServiceSource {
    /// A source with the image reference and the build a service declares.
    #[must_use]
    pub const fn new(image: Option<ImageRef>, build: Option<Build>) -> Self {
        Self { image, build }
    }

    /// The image reference, when the service declares one.
    #[must_use]
    pub const fn image(&self) -> Option<&ImageRef> {
        self.image.as_ref()
    }

    /// The build, when the service is built from the repository.
    #[must_use]
    pub const fn build(&self) -> Option<&Build> {
        self.build.as_ref()
    }
}

collection!(
    /// The `env_file` entries of a service, relative to the stack directory.
    EnvFiles,
    RepoPath
);
collection!(
    /// The mounts of a service.
    Mounts,
    Mount
);
collection!(
    /// The services of a compose file, in name order.
    Services,
    Service
);
collection!(
    /// The top-level named volumes of a compose file.
    Volumes,
    VolumeName
);

/// One compose service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    name: ServiceName,
    source: ServiceSource,
    env_files: EnvFiles,
    mounts: Mounts,
}

impl Service {
    /// A service with no env files and no mounts.
    #[must_use]
    pub fn new(name: ServiceName, source: ServiceSource) -> Self {
        Self {
            name,
            source,
            env_files: EnvFiles::default(),
            mounts: Mounts::default(),
        }
    }

    /// The same service with `env_files`.
    #[must_use]
    pub fn with_env_files(self, env_files: EnvFiles) -> Self {
        Self { env_files, ..self }
    }

    /// The same service with `mounts`.
    #[must_use]
    pub fn with_mounts(self, mounts: Mounts) -> Self {
        Self { mounts, ..self }
    }

    /// The service name.
    #[must_use]
    pub const fn name(&self) -> &ServiceName {
        &self.name
    }

    /// Where the service gets its image.
    #[must_use]
    pub const fn source(&self) -> &ServiceSource {
        &self.source
    }

    /// The `env_file` entries, relative to the stack directory.
    #[must_use]
    pub const fn env_files(&self) -> &EnvFiles {
        &self.env_files
    }

    /// The mounts.
    #[must_use]
    pub const fn mounts(&self) -> &Mounts {
        &self.mounts
    }
}

/// The services and top-level volumes of a compose file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComposeSummary {
    services: Services,
    volumes: Volumes,
}

impl ComposeSummary {
    /// A summary of `services` and `volumes`.
    #[must_use]
    pub const fn new(services: Services, volumes: Volumes) -> Self {
        Self { services, volumes }
    }

    /// The services, in name order.
    #[must_use]
    pub const fn services(&self) -> &Services {
        &self.services
    }

    /// The top-level named volumes.
    #[must_use]
    pub const fn volumes(&self) -> &Volumes {
        &self.volumes
    }
}
