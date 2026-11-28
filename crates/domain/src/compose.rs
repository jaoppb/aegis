//! The part of a compose file the validation rules reason about.

use crate::value::{
    ContainerPath, DeclaredPath, HostPath, ImageRef, RepoPath, SecretName, ServiceName, VolumeName,
    collection,
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
    additional: AdditionalContexts,
}

impl Build {
    /// A build of `context`, with `dockerfile` when it is not the default.
    #[must_use]
    pub fn new(context: DeclaredPath, dockerfile: Option<DeclaredPath>) -> Self {
        Self {
            context,
            dockerfile,
            additional: AdditionalContexts::default(),
        }
    }

    /// The same build with `additional` named contexts.
    #[must_use]
    pub fn with_additional_contexts(self, additional: AdditionalContexts) -> Self {
        Self { additional, ..self }
    }

    /// The extra named build contexts that are paths.
    #[must_use]
    pub const fn additional_contexts(&self) -> &AdditionalContexts {
        &self.additional
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
/// One top-level `secrets` entry that reads a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretFile {
    name: SecretName,
    path: RepoPath,
}

impl SecretFile {
    /// The secret `name` read from `path`, relative to the stack directory.
    #[must_use]
    pub const fn new(name: SecretName, path: RepoPath) -> Self {
        Self { name, path }
    }

    /// The name the compose file gives the secret.
    #[must_use]
    pub const fn name(&self) -> &SecretName {
        &self.name
    }

    /// The path, relative to the stack directory.
    #[must_use]
    pub const fn path(&self) -> &RepoPath {
        &self.path
    }
}

collection!(
    /// The top-level `secrets` of a compose file that read a file.
    SecretFiles,
    SecretFile
);
/// Why a compose file points at another file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceKind {
    /// An `include` entry: Compose merges the whole file in.
    Include,
    /// A service `extends.file`: Compose merges the service in.
    Extends,
    /// A top-level `configs` file.
    Config,
}

/// A file a compose file points at, other than env and secret files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileReference {
    path: RepoPath,
    kind: ReferenceKind,
}

impl FileReference {
    /// A reference to `path` of `kind`, relative to the stack directory.
    #[must_use]
    pub const fn new(path: RepoPath, kind: ReferenceKind) -> Self {
        Self { path, kind }
    }

    /// The path, relative to the stack directory.
    #[must_use]
    pub const fn path(&self) -> &RepoPath {
        &self.path
    }

    /// Why the file is referenced.
    #[must_use]
    pub const fn kind(&self) -> ReferenceKind {
        self.kind
    }
}

collection!(
    /// The `include`, `extends` and `configs` files of a compose file.
    ReferencedFiles,
    FileReference
);
collection!(
    /// The extra named build contexts of a service (`additional_contexts`).
    AdditionalContexts,
    DeclaredPath
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
    shape: Shape,
    env_files: EnvFiles,
    mounts: Mounts,
}

/// What a service is made from and how it is networked, kept together because both decide how the
/// service starts.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Shape {
    source: ServiceSource,
    networking: Networking,
}

impl Service {
    /// A service with no env files and no mounts.
    #[must_use]
    pub fn new(name: ServiceName, source: ServiceSource) -> Self {
        Self {
            name,
            shape: Shape {
                source,
                networking: Networking::default(),
            },
            env_files: EnvFiles::default(),
            mounts: Mounts::default(),
        }
    }

    /// The same service with `networking`.
    #[must_use]
    pub fn with_networking(self, networking: Networking) -> Self {
        let shape = Shape {
            networking,
            ..self.shape
        };
        Self { shape, ..self }
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
        &self.shape.source
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

    /// Whether the service sets its own network.
    #[must_use]
    pub const fn networking(&self) -> Networking {
        self.shape.networking
    }
}

/// Whether a service sets its own network, which a service that joins the tailnet node's network
/// namespace may not.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Networking {
    /// The service sets none of `network_mode`, `networks`, `ports` and `hostname`.
    #[default]
    Default,
    /// The service sets at least one of them.
    Own,
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
