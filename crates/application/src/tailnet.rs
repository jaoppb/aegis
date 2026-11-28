//! Tailnet enrolment: the ports for minting node keys and deleting nodes, and the rule that a key
//! is minted only for a sidecar that has no state yet.

use std::fmt;
use std::future::Future;

use aegis_domain::{HostName, TailnetTags, ValueError};

use crate::source::Reason;

/// A Tailscale auth key. It is a secret: it has no `Display`, and its `Debug` hides the value.
#[derive(Clone, PartialEq, Eq)]
pub struct AuthKey(String);

impl AuthKey {
    /// Wrap the key text Tailscale returned.
    #[must_use]
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    /// The key text, for the one place that hands it to a sidecar.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AuthKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AuthKey(<redacted>)")
    }
}

/// The identifier Tailscale gives a device. It is restricted to letters, digits and `-`, so it can
/// never alter the path of an API call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceId(String);

const fn is_id_char(candidate: char) -> bool {
    candidate.is_ascii_alphanumeric() || candidate == '-'
}

fn validate_id(value: &str) -> Result<(), ValueError> {
    if value.is_empty() {
        return Err(ValueError::Empty);
    }
    let forbidden = value.chars().find(|candidate| !is_id_char(*candidate));
    forbidden.map_or(Ok(()), |found| Err(ValueError::ForbiddenChar(found)))
}

impl DeviceId {
    /// Validate `value` and wrap it.
    ///
    /// # Errors
    ///
    /// Returns a [`ValueError`] when `value` is empty or holds a character other than an ASCII
    /// letter, a digit or `-`.
    pub fn new(value: impl Into<String>) -> Result<Self, ValueError> {
        let value = value.into();
        validate_id(&value)?;
        Ok(Self(value))
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A device to delete: named by its Tailscale ID, or by the hostname of its node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceRef {
    /// The device with this ID.
    Identifier(DeviceId),
    /// The one device with this hostname.
    Hostname(HostName),
}

/// A call to the Tailscale API did not succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TailnetError {
    /// A key needs at least one tag, and none was given.
    NoTags,
    /// No device matches.
    NoSuchDevice,
    /// A hostname matches more than one device, so nothing was deleted.
    AmbiguousHostname,
    /// Tailscale refused the request, for example because the OAuth client lacks a scope.
    Rejected(Reason),
    /// The call could not be made or its answer could not be read.
    Unavailable(Reason),
}

impl fmt::Display for TailnetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoTags => formatter.write_str("an auth key needs at least one tag"),
            Self::NoSuchDevice => formatter.write_str("no such device"),
            Self::AmbiguousHostname => formatter.write_str("the hostname matches several devices"),
            Self::Rejected(reason) => write!(formatter, "tailscale refused the request: {reason}"),
            Self::Unavailable(reason) => write!(formatter, "tailscale call failed: {reason}"),
        }
    }
}

impl std::error::Error for TailnetError {}

/// The Tailscale API: mints single-use, pre-authorized, non-ephemeral, tagged auth keys.
pub trait KeyMinter {
    /// A new auth key that tags its node with `tags`.
    ///
    /// # Errors
    ///
    /// Returns [`TailnetError::NoTags`] for an empty `tags`, and another [`TailnetError`] when
    /// Tailscale refuses or cannot be reached.
    fn mint(
        &self,
        tags: &TailnetTags,
    ) -> impl Future<Output = Result<AuthKey, TailnetError>> + Send;
}

/// The Tailscale API: removes nodes from the tailnet.
pub trait NodeRemover {
    /// Delete the device `node`.
    ///
    /// # Errors
    ///
    /// Returns [`TailnetError::NoSuchDevice`] when nothing matches,
    /// [`TailnetError::AmbiguousHostname`] when a hostname matches several devices, and another
    /// [`TailnetError`] when Tailscale refuses or cannot be reached.
    fn remove(&self, node: &DeviceRef) -> impl Future<Output = Result<(), TailnetError>> + Send;
}

/// Whether a sidecar's state volume holds a node identity yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateVolume {
    /// Nothing is stored: the node has never joined the tailnet.
    Empty,
    /// A node identity is stored, so a restart reuses it.
    Populated,
}

/// What the agent reports about a sidecar's state volume.
pub trait SidecarState {
    /// The state of the sidecar for `node`.
    ///
    /// # Errors
    ///
    /// Returns a [`Reason`] when the state could not be read.
    fn volume(&self, node: &HostName) -> impl Future<Output = Result<StateVolume, Reason>> + Send;
}

/// What [`Enrollment::enroll`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Enrolled {
    /// The sidecar had no state, so this key was minted for it.
    Minted(AuthKey),
    /// The sidecar already has state: no key was minted and Tailscale was not called.
    AlreadyEnrolled,
}

/// Enrolment did not complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnrollError {
    /// The sidecar's state could not be read, so no key was minted.
    State(Reason),
    /// Minting the key failed.
    Mint(TailnetError),
}

impl fmt::Display for EnrollError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::State(reason) => write!(formatter, "cannot read the sidecar state: {reason}"),
            Self::Mint(error) => write!(formatter, "cannot mint a key: {error}"),
        }
    }
}

impl std::error::Error for EnrollError {}

/// Mints a key for a node only while its sidecar has no state, so restarts never create nodes.
#[derive(Debug)]
pub struct Enrollment<State, Minter> {
    state: State,
    minter: Minter,
}

impl<State: SidecarState + Sync, Minter: KeyMinter + Sync> Enrollment<State, Minter> {
    /// An enrolment that reads sidecar state from `state` and mints keys with `minter`.
    #[must_use]
    pub const fn new(state: State, minter: Minter) -> Self {
        Self { state, minter }
    }

    /// Mint a key tagged `tags` for `node`, unless its sidecar already has state.
    ///
    /// # Errors
    ///
    /// Returns [`EnrollError::State`] when the state cannot be read (nothing is minted), and
    /// [`EnrollError::Mint`] when the key cannot be minted.
    pub async fn enroll(
        &self,
        node: &HostName,
        tags: &TailnetTags,
    ) -> Result<Enrolled, EnrollError> {
        let volume = self.state.volume(node).await.map_err(EnrollError::State)?;
        match volume {
            StateVolume::Populated => Ok(Enrolled::AlreadyEnrolled),
            StateVolume::Empty => self.mint_key(tags).await,
        }
    }

    async fn mint_key(&self, tags: &TailnetTags) -> Result<Enrolled, EnrollError> {
        let key = self.minter.mint(tags).await.map_err(EnrollError::Mint)?;
        Ok(Enrolled::Minted(key))
    }
}
