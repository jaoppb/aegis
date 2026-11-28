//! Tailnet access control: who may call an API, decided from the peer's ACL tags.

use std::fmt;
use std::future::Future;

use aegis_domain::{AegisRole, Logins, PeerIdentity, TailnetTag, ValueError};

use crate::source::Reason;

/// The network address of a peer as the transport saw it, such as `100.64.0.7:41234`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerAddress(String);

impl PeerAddress {
    /// The address rendered as `text`.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }
}

impl fmt::Display for PeerAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A peer could not be resolved to an identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhoIsError {
    /// The address belongs to no tailnet node.
    Unknown,
    /// The lookup itself failed, for example because the Tailscale daemon is not reachable.
    Unavailable(Reason),
}

impl fmt::Display for WhoIsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown => formatter.write_str("the address is not a tailnet node"),
            Self::Unavailable(reason) => write!(formatter, "the whois lookup failed: {reason}"),
        }
    }
}

impl std::error::Error for WhoIsError {}

/// The Tailscale local API: resolves a peer address to the node that owns it.
pub trait WhoIs {
    /// The identity of the node behind `peer`.
    ///
    /// # Errors
    ///
    /// Returns [`WhoIsError::Unknown`] when no tailnet node owns the address, and
    /// [`WhoIsError::Unavailable`] when the lookup could not be made.
    fn identify(
        &self,
        peer: &PeerAddress,
    ) -> impl Future<Output = Result<PeerIdentity, WhoIsError>> + Send;
}

/// A request was not let through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessError {
    /// The peer is not a tailnet node, or does not carry the required tag.
    Denied,
    /// The peer could not be checked, so it is refused: access fails closed.
    Unavailable(Reason),
}

impl fmt::Display for AccessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Denied => formatter.write_str("the peer does not carry the required tag"),
            Self::Unavailable(reason) => write!(formatter, "cannot check the peer: {reason}"),
        }
    }
}

impl std::error::Error for AccessError {}

/// Who a route lets through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admit {
    /// Only the node that carries the server's required ACL tag.
    Tagged,
    /// Only an operator: a person's untagged node, listed in the operators.
    Operator,
    /// Either of the above.
    Either,
}

/// Lets a peer through when it carries one required ACL tag or, on the routes that say so, when it
/// is an operator.
#[derive(Debug)]
pub struct Authorizer<Lookup> {
    whois: Lookup,
    required: TailnetTag,
    operators: Logins,
}

impl<Lookup: WhoIs + Sync> Authorizer<Lookup> {
    /// An authorizer that resolves peers with `whois` and requires `required`. It knows no
    /// operator until [`Authorizer::with_operators`] names them.
    #[must_use]
    pub fn new(whois: Lookup, required: TailnetTag) -> Self {
        Self {
            whois,
            required,
            operators: Logins::default(),
        }
    }

    /// An authorizer for a server with `role`, which admits only the other side's tag.
    ///
    /// # Errors
    ///
    /// Returns a [`ValueError`] only if the built-in tag name were invalid.
    pub fn for_server(whois: Lookup, role: AegisRole) -> Result<Self, ValueError> {
        Ok(Self::new(whois, role.caller_tag()?))
    }

    /// The same authorizer that also admits the people `operators` on the routes that allow them.
    /// Without it no operator is admitted: access fails closed.
    #[must_use]
    pub fn with_operators(self, operators: Logins) -> Self {
        Self { operators, ..self }
    }

    /// Check that `peer` carries the required tag.
    ///
    /// # Errors
    ///
    /// Returns [`AccessError::Denied`] for an unknown, untagged or wrongly tagged peer, and
    /// [`AccessError::Unavailable`] when the lookup failed.
    pub async fn authorize(&self, peer: &PeerAddress) -> Result<(), AccessError> {
        self.authorize_for(peer, Admit::Tagged).await
    }

    /// Check that `peer` is one of the callers `admit` names.
    ///
    /// # Errors
    ///
    /// The same as [`Authorizer::authorize`]; an operator on a route that does not admit
    /// operators is denied.
    pub async fn authorize_for(&self, peer: &PeerAddress, admit: Admit) -> Result<(), AccessError> {
        let identity = self.identify(peer).await?;
        self.admits(&identity, admit)
    }

    /// Resolve `peer` to the node behind it.
    ///
    /// # Errors
    ///
    /// Returns [`AccessError::Denied`] for an address that is no tailnet node, and
    /// [`AccessError::Unavailable`] when the lookup failed.
    pub async fn identify(&self, peer: &PeerAddress) -> Result<PeerIdentity, AccessError> {
        self.whois.identify(peer).await.map_err(refuse)
    }

    /// Check that `identity` is one of the callers `admit` names.
    ///
    /// # Errors
    ///
    /// Returns [`AccessError::Denied`] when it is not.
    pub fn admits(&self, identity: &PeerIdentity, admit: Admit) -> Result<(), AccessError> {
        let operator = identity.is_operator(&self.operators);
        let allowed = match admit {
            Admit::Tagged => self.is_tagged(identity),
            Admit::Operator => operator,
            Admit::Either => self.is_tagged(identity) || operator,
        };
        allowed.then_some(()).ok_or(AccessError::Denied)
    }

    fn is_tagged(&self, identity: &PeerIdentity) -> bool {
        identity.has_tag(&self.required)
    }
}

fn refuse(error: WhoIsError) -> AccessError {
    match error {
        WhoIsError::Unknown => AccessError::Denied,
        WhoIsError::Unavailable(reason) => AccessError::Unavailable(reason),
    }
}
