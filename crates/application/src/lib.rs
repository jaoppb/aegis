//! Aegis application layer: the ports the adapters implement, the repository loader and the
//! validation rules built on them. It does no IO.

pub mod access;
pub mod loader;
pub mod outcome;
pub mod parse;
pub mod source;
pub mod tailnet;
pub mod validate;

pub use access::{AccessError, Authorizer, PeerAddress, WhoIs, WhoIsError};
pub use loader::load_desired_state;
pub use outcome::Outcome;
pub use parse::{ParseError, StackParser};
pub use source::{DirEntry, EntryKind, EntryName, Reason, RepoSource, SourceError};
pub use tailnet::{
    AuthKey, DeviceId, DeviceRef, EnrollError, Enrolled, Enrollment, KeyMinter, NodeRemover,
    SidecarState, StateVolume, TagError, TailnetError,
};
pub use validate::{EncryptionCheck, validate_desired_state};
