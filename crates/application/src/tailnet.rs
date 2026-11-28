//! Tailnet enrolment: the ports for minting node keys and deleting nodes, and the rule that a key
//! is minted only for a sidecar that has no state yet.

use std::fmt;
use std::future::Future;

use aegis_domain::{AegisRole, HostName, TailnetTag, TailnetTags, ValueError};

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
    /// The base URL would send the OAuth secret over an unprotected connection.
    InsecureBase,
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
            Self::InsecureBase => formatter.write_str("the API URL must be https or loopback"),
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
    /// Returns a [`TailnetError`] when Tailscale refuses (it refuses an empty `tags`) or cannot be
    /// reached. The minter does not judge which tags are acceptable: [`Enrollment`] does.
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

/// What [`Enrollment::enroll`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Enrolled {
    /// The sidecar had no state, so this key was minted for it.
    Minted(AuthKey),
    /// The sidecar already has state: no key was minted and Tailscale was not called.
    AlreadyEnrolled,
}

/// Why the tags a stack asks for cannot be put on a node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagError {
    /// No tag was given, and Tailscale needs a tag to own the node.
    Empty,
    /// The tag is one of Aegis's own role tags, which the API authorizer trusts.
    Reserved(TailnetTag),
    /// The operator did not allow this tag for stack nodes.
    NotAllowed(TailnetTag),
}

impl fmt::Display for TagError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("a node needs at least one tag"),
            Self::Reserved(tag) => write!(formatter, "{tag} is reserved for Aegis itself"),
            Self::NotAllowed(tag) => write!(formatter, "{tag} is not allowed for stack nodes"),
        }
    }
}

impl std::error::Error for TagError {}

fn check_tags(tags: &TailnetTags) -> Result<(), TagError> {
    if tags.is_empty() {
        return Err(TagError::Empty);
    }
    let claimed = tags.iter().find(|tag| AegisRole::is_role_tag(tag));
    claimed.map_or(Ok(()), |tag| Err(TagError::Reserved(tag.clone())))
}

/// Enrolment did not complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnrollError {
    /// The stack's tags are not acceptable, so nothing was read or minted.
    Tags(TagError),
    /// Minting the key failed.
    Mint(TailnetError),
    /// The name of the tailnet could not be discovered.
    Tailnet(TailnetError),
    /// The controller could not be asked, or refused, so nothing was enrolled.
    Controller(Reason),
}

impl fmt::Display for EnrollError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tags(error) => write!(formatter, "cannot tag the node: {error}"),
            Self::Mint(error) => write!(formatter, "cannot mint a key: {error}"),
            Self::Tailnet(error) => write!(formatter, "cannot discover the tailnet: {error}"),
            Self::Controller(reason) => write!(formatter, "the controller failed: {reason}"),
        }
    }
}

impl std::error::Error for EnrollError {}

/// Mints a key for a node only while its sidecar has no state, so restarts never create nodes.
#[derive(Debug)]
pub struct Enrollment<Minter> {
    minter: Minter,
}

impl<Minter> Enrollment<Minter> {
    /// An enrolment that mints keys with `minter`.
    #[must_use]
    pub const fn new(minter: Minter) -> Self {
        Self { minter }
    }
}

impl<Minter: KeyMinter + Sync> Enrollment<Minter> {
    /// Mint a key tagged `tags` for a node whose sidecar state volume is `state`, unless it
    /// already has state.
    ///
    /// # Errors
    ///
    /// Returns [`EnrollError::Tags`] when `tags` is empty or names an Aegis role tag (a stack
    /// must not enrol a node as the controller or an agent), and [`EnrollError::Mint`] when the
    /// key cannot be minted.
    pub async fn enroll(
        &self,
        tags: &TailnetTags,
        state: StateVolume,
    ) -> Result<Enrolled, EnrollError> {
        check_tags(tags).map_err(EnrollError::Tags)?;
        self.enroll_tagged(tags, state)
            .await
            .map_err(EnrollError::Mint)
    }

    /// The same enrolment without the tag check, for the nodes of Aegis itself, whose role tags
    /// a stack may not claim. There is still one minting path: only a sidecar with no state mints.
    pub(crate) async fn enroll_tagged(
        &self,
        tags: &TailnetTags,
        state: StateVolume,
    ) -> Result<Enrolled, TailnetError> {
        match state {
            StateVolume::Populated => Ok(Enrolled::AlreadyEnrolled),
            StateVolume::Empty => self.mint_key(tags).await,
        }
    }

    async fn mint_key(&self, tags: &TailnetTags) -> Result<Enrolled, TailnetError> {
        let key = self.minter.mint(tags).await?;
        Ok(Enrolled::Minted(key))
    }
}
