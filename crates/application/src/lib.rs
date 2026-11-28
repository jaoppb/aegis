//! Aegis application layer: the ports the adapters implement, the repository loader and the
//! validation rules built on them. It does no IO.

pub mod access;
pub mod deploy;
pub mod enroll;
pub mod loader;
pub mod outcome;
pub mod parse;
pub mod reconcile;
pub mod secrets;
pub mod sidecar;
pub mod source;
pub mod tailnet;
pub mod validate;

pub use access::{AccessError, Authorizer, PeerAddress, WhoIs, WhoIsError};
pub use deploy::{
    BuiltServices, Checkout, ComposeError, ComposeRunner, DeployRequest, Environment, ExtraFiles,
    Extras, GitError, Target,
};
pub use enroll::{Enroller, EnrollmentRequest, Grant, SidecarEnroller, StateProbe, TailnetNames};
pub use loader::load_desired_state;
pub use outcome::Outcome;
pub use parse::{ParseError, StackParser};
pub use reconcile::{Reconcile, ReconcileError, ReconcilePorts, Reconciler, Repository};
pub use secrets::{DecryptError, SecretDecryptor, SecretStore, Slot, StoreError};
pub use sidecar::{
    Caddyfile, Exposure, KEY_VARIABLE, KeyUse, OverrideYaml, RenderedSidecar, STATE_DIRECTORY,
    STATE_FILE, STATE_VOLUME, SidecarSpec, TAILSCALE_IMAGE, is_sidecar_service, is_sidecar_volume,
    state_volume_of,
};
pub use source::{DirEntry, EntryKind, EntryName, Reason, RepoSource, SourceError};
pub use tailnet::{
    AuthKey, DeviceId, DeviceRef, EnrollError, Enrolled, Enrollment, KeyMinter, NodeRemover,
    StateVolume, TagError, TailnetError,
};
pub use validate::{EncryptionCheck, validate_desired_state};
