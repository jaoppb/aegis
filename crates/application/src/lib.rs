//! Aegis application layer: the ports the adapters implement, the repository loader and the
//! validation rules built on them. It does no IO.

pub mod access;
pub mod bootstrap;
pub mod deploy;
pub mod loader;
pub mod outcome;
pub mod parse;
pub mod reconcile;
pub mod render;
pub mod secrets;
pub mod source;
pub mod tailnet;
pub mod validate;

pub use access::{AccessError, Authorizer, PeerAddress, WhoIs, WhoIsError};
pub use bootstrap::{
    AgeRecipient, Bootstrap, BootstrapError, BootstrapPlan, BootstrapReport, ControllerChoice,
    ControllerUrl, Existing, FileText, HostFiles, HostKey, HostSide, ImageTag, Joined, NodeKeys,
    Outcomes, RenderedFile, RepoUrl, SecretEncryptor, Sources, StackOutcome, StackRunner, Written,
    sidecar_volume,
};
pub use deploy::{
    BuiltServices, Checkout, ComposeError, ComposeRunner, DeployRequest, Environment, ExtraFiles,
    Extras, GitError, Target,
};
pub use loader::load_desired_state;
pub use outcome::Outcome;
pub use parse::{ParseError, StackParser};
pub use reconcile::{Reconcile, ReconcileError, ReconcilePorts, Reconciler, Repository};
pub use secrets::{DecryptError, SecretDecryptor, SecretStore, Slot, StoreError};
pub use source::{DirEntry, EntryKind, EntryName, Reason, RepoSource, SourceError};
pub use tailnet::{
    AuthKey, DeviceId, DeviceRef, EnrollError, Enrolled, Enrollment, KeyMinter, NodeRemover,
    SidecarState, StateVolume, TagError, TailnetError,
};
pub use validate::{EncryptionCheck, validate_desired_state};
