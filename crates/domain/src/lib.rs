//! Aegis domain layer: the desired state a repository declares.
//!
//! The domain is pure. It holds validated value types, the compose summary the validation rules
//! reason about and the stack policy, and knows nothing about files, wire formats or processes.

pub mod compose;
pub mod peer;
pub mod policy;
pub mod state;
pub mod value;

pub use compose::{
    Access, Build, ComposeSummary, EnvFiles, Mount, Mounts, Service, ServiceSource, Services,
    Volumes,
};
pub use peer::{AegisRole, PeerIdentity};
pub use policy::{Expose, StackPolicy, StackState, TailnetTags, UpdatePolicy};
pub use state::{
    DesiredState, Host, Hosts, Stack, StackDefinition, StackIdentity, StackKind, Stacks,
};
pub use value::{
    ComposeSource, ContainerPath, HostName, HostPath, ImageRef, Port, RepoPath, ServiceName,
    StackName, TailnetTag, ValueError, VolumeName,
};
