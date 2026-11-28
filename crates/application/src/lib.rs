//! Aegis application layer: the ports the adapters implement, the repository loader and the
//! validation rules built on them. It does no IO.

pub mod access;
pub mod deploy;
pub mod fleet;
pub mod loader;
pub mod outcome;
pub mod parse;
pub mod poll;
pub mod reconcile;
pub mod secrets;
pub mod source;
pub mod tailnet;
pub mod validate;

pub use access::{AccessError, Authorizer, PeerAddress, WhoIs, WhoIsError};
pub use deploy::{
    BuiltServices, Checkout, ComposeError, ComposeRunner, DeployRequest, Environment, ExtraFiles,
    Extras, GitError, Target, Upstream,
};
pub use fleet::{Agents, Fleet, Goal, Invalid, Notice, Observation, Order};
pub use loader::load_desired_state;
pub use outcome::Outcome;
pub use parse::{ParseError, StackParser};
pub use poll::Poller;
pub use reconcile::{Reconcile, ReconcileError, ReconcilePorts, Reconciler, Repository};
pub use secrets::{DecryptError, SecretDecryptor, SecretStore, Slot, StoreError};
pub use source::{DirEntry, EntryKind, EntryName, Reason, RepoSource, SourceError};
pub use tailnet::{
    AuthKey, DeviceId, DeviceRef, EnrollError, Enrolled, Enrollment, KeyMinter, NodeRemover,
    SidecarState, StateVolume, TagError, TailnetError,
};
pub use validate::{EncryptionCheck, validate_desired_state};
