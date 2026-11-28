//! Aegis domain layer: the desired state a repository declares.
//!
//! The domain is pure. It holds validated value types, the compose summary the validation rules
//! reason about and the stack policy, and knows nothing about files, wire formats or processes.

pub mod compose;
pub mod deployment;
pub mod peer;
pub mod policy;
pub mod state;
pub mod value;
pub mod violation;

pub use compose::{
    Access, AdditionalContexts, Build, ComposeSummary, EnvFile, EnvFiles, FileReference, Mount,
    Mounts, Presence, ReferenceKind, ReferencedFiles, SecretFiles, Service, ServiceSource,
    Services, Volumes,
};
pub use deployment::{
    CommitSha, Deployment, Detail, Origin, ProjectName, Report, StackReport, StackReports,
};
pub use peer::{AegisRole, PeerIdentity};
pub use policy::{Expose, StackPolicy, StackState, TailnetTags, UpdatePolicy};
pub use state::{
    DesiredState, Host, Hosts, Stack, StackDefinition, StackIdentity, StackKind, Stacks,
};
pub use value::{
    ComposeSource, ContainerPath, DeclaredPath, HostName, HostPath, ImageRef, Line, Port, RepoPath,
    ServiceName, StackName, TailnetTag, ValueError, VolumeName,
};
pub use violation::{FileRole, Location, Problem, Rejection, Rejections, Violation, Violations};
