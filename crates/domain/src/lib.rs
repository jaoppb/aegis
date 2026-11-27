//! Aegis domain layer: the desired state a repository declares.
//!
//! The domain is pure. It holds validated value types, the compose summary the validation rules
//! reason about and the stack policy, and knows nothing about files, wire formats or processes.

pub mod compose;
pub mod policy;
pub mod state;
pub mod value;
pub mod violation;

pub use compose::{
    Access, Build, ComposeSummary, EnvFile, EnvFiles, Mount, Mounts, Presence, ReferencedFiles,
    SecretFiles, Service, ServiceSource, Services, Volumes,
};
pub use policy::{Expose, StackPolicy, StackState, TailnetTags, UpdatePolicy};
pub use state::{
    DesiredState, Host, Hosts, Stack, StackDefinition, StackIdentity, StackKind, Stacks,
};
pub use value::{
    ComposeSource, ContainerPath, DeclaredPath, HostName, HostPath, ImageRef, Line, Port, RepoPath,
    ServiceName, StackName, TailnetTag, ValueError, VolumeName,
};
pub use violation::{FileRole, Location, Problem, Rejection, Rejections, Violation, Violations};
