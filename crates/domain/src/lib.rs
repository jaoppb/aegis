//! Aegis domain layer: the desired state a repository declares.
//!
//! The domain is pure. It holds validated value types, the compose summary the validation rules
//! reason about and the stack policy, and knows nothing about files, wire formats or processes.

pub mod compose;
pub mod deployment;
pub mod peer;
pub mod policy;
pub mod removal;
pub mod state;
pub mod value;
pub mod violation;

pub use compose::{
    Access, AdditionalContexts, Build, ComposeSummary, EnvFile, EnvFiles, FileReference, Mount,
    Mounts, Networking, Presence, ReferenceKind, ReferencedFiles, SecretFile, SecretFiles, Service,
    ServiceSource, Services, Volumes,
};
pub use deployment::{
    CommitSha, Deployment, Detail, HostNames, Origin, ProjectName, Report, ReservedStacks,
    StackReport, StackReports,
};
pub use peer::{AegisRole, Login, Logins, PeerIdentity};
pub use policy::{Expose, Node, Reach, StackPolicy, StackState, TailnetTags, UpdatePolicy};
pub use removal::{
    HostPaths, Leftover, Leftovers, ProjectNames, Remains, Tombstone, TombstoneError, exposed_node,
    mounted_paths,
};
pub use state::{
    DesiredState, Host, Hosts, Stack, StackDefinition, StackIdentity, StackKind, Stacks,
};
pub use value::{
    ComposeSource, ContainerPath, DeclaredPath, HostName, HostPath, ImageRef, Line, NodeLabel,
    Port, RepoPath, Secret, SecretName, ServiceName, SlotName, StackName, TailnetName, TailnetTag,
    ValueError, VolumeName,
};
pub use violation::{FileRole, Location, Problem, Rejection, Rejections, Violation, Violations};
