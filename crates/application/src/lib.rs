//! Aegis application layer: the ports the adapters implement, the repository loader and the
//! validation rules built on them. It does no IO.

pub mod access;
pub mod deploy;
pub mod loader;
pub mod outcome;
pub mod parse;
pub mod reconcile;
pub mod source;
pub mod validate;

pub use access::{AccessError, Authorizer, PeerAddress, WhoIs, WhoIsError};
pub use deploy::{
    BuiltServices, Checkout, ComposeError, ComposeRunner, DeployRequest, Environment, ExtraFiles,
    Extras, GitError, Target,
};
pub use loader::load_desired_state;
pub use outcome::Outcome;
pub use parse::{ParseError, StackParser};
pub use reconcile::{Ports, Reconcile, ReconcileError, Reconciler, Repository};
pub use source::{DirEntry, EntryKind, EntryName, Reason, RepoSource, SourceError};
pub use validate::{EncryptionCheck, validate_desired_state};
