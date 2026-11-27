//! Aegis domain layer: the desired state a repository declares.
//!
//! The domain is pure. It holds validated value types, the compose summary the validation rules
//! reason about and the stack policy, and knows nothing about files, wire formats or processes.

pub mod compose;
pub mod policy;
pub mod state;
pub mod value;

pub use compose::{Access, Build, ComposeSummary, Mount, Service};
pub use policy::{Expose, StackPolicy, StackState, UpdatePolicy};
pub use state::{DesiredState, Host, Stack, StackKind};
pub use value::{
    ContainerPath, HostName, HostPath, Port, RepoPath, ServiceName, StackName, TailnetTag,
    ValueError, VolumeName,
};
