//! The part of a compose file the validation rules reason about.

use crate::value::{
    ContainerPath, DeclaredPath, HostPath, ImageRef, RepoPath, ServiceName, VolumeName, collection,
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

fn real_segments(path: &str) -> impl Iterator<Item = &str> {
    path.split('/')
        .filter(|segment| !matches!(*segment, "" | "."))
}

fn climbs_out(depth: usize, dockerfile: &str) -> bool {
    if dockerfile.starts_with('/') {
        return true;
    }
    real_segments(dockerfile)
        .try_fold(depth, step_into)
        .is_none()
}

fn step_into(depth: usize, segment: &str) -> Option<usize> {
    if segment == ".." {
        return depth.checked_sub(1);
    }
    Some(depth.saturating_add(1))
}

/// A service image built from the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    context: DeclaredPath,
    dockerfile: Option<DeclaredPath>,
}

impl Build {
    /// A build of `context`, with `dockerfile` when it is not the default.
    #[must_use]
    pub const fn new(context: DeclaredPath, dockerfile: Option<DeclaredPath>) -> Self {
        Self {
            context,
            dockerfile,
        }
    }

    /// The build context, relative to the stack directory.
    #[must_use]
    pub const fn context(&self) -> &DeclaredPath {
        &self.context
    }

    /// The build context inside the stack directory, or `None` when it is absolute or climbs out.
    #[must_use]
    pub fn context_inside_stack(&self) -> Option<RepoPath> {
        self.context.inside_stack()
    }

    /// Whether the Dockerfile, resolved against the context, leaves the stack directory.
    #[must_use]
    pub fn dockerfile_escapes(&self) -> bool {
        let depth = self.context_depth();
        self.dockerfile
            .as_ref()
            .is_some_and(|dockerfile| climbs_out(depth, dockerfile.as_str()))
    }

    fn context_depth(&self) -> usize {
        let inside = self.context_inside_stack();
        inside.map_or(0, |context| real_segments(context.as_str()).count())
    }

    /// The Dockerfile, relative to the context, when not the default.
    #[must_use]
    pub const fn dockerfile(&self) -> Option<&DeclaredPath> {
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

/// Whether a referenced file has to exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    /// The file must exist.
    Required,
    /// The file may be absent (`required: false`).
    Optional,
}

/// One `env_file` entry of a service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvFile {
    path: RepoPath,
    presence: Presence,
}

impl EnvFile {
    /// The env file at `path`, relative to the stack directory.
    #[must_use]
    pub const fn new(path: RepoPath, presence: Presence) -> Self {
        Self { path, presence }
    }

    /// The path, relative to the stack directory.
    #[must_use]
    pub const fn path(&self) -> &RepoPath {
        &self.path
    }

    /// Whether the file has to exist.
    #[must_use]
    pub const fn presence(&self) -> Presence {
        self.presence
    }
}

collection!(
    /// The `env_file` entries of a service.
    EnvFiles,
    EnvFile
);
collection!(
    /// The files the top-level `secrets` of a compose file read, relative to the stack directory.
    SecretFiles,
    RepoPath
);
collection!(
    /// Other files a compose file pulls in (`include`, `extends`, `configs`), relative to the
    /// stack directory.
    ReferencedFiles,
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

/// The services, volumes and file references of a compose file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComposeSummary {
    services: Services,
    volumes: Volumes,
    secret_files: SecretFiles,
    referenced_files: ReferencedFiles,
}

impl ComposeSummary {
    /// A summary of `services` and `volumes` with no file references.
    #[must_use]
    pub fn new(services: Services, volumes: Volumes) -> Self {
        Self {
            services,
            volumes,
            secret_files: SecretFiles::default(),
            referenced_files: ReferencedFiles::default(),
        }
    }

    /// The same summary with `secret_files`.
    #[must_use]
    pub fn with_secret_files(self, secret_files: SecretFiles) -> Self {
        Self {
            secret_files,
            ..self
        }
    }

    /// The same summary with `referenced_files`.
    #[must_use]
    pub fn with_referenced_files(self, referenced_files: ReferencedFiles) -> Self {
        Self {
            referenced_files,
            ..self
        }
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

    /// The files the top-level `secrets` read.
    #[must_use]
    pub const fn secret_files(&self) -> &SecretFiles {
        &self.secret_files
    }

    /// The files pulled in by `include`, `extends` and `configs`.
    #[must_use]
    pub const fn referenced_files(&self) -> &ReferencedFiles {
        &self.referenced_files
    }
}
