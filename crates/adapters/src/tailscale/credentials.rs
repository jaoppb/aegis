//! The OAuth client the controller authenticates as. The secret is never printed.

use std::fmt;

/// The id of the Tailscale OAuth client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientId(String);

impl ClientId {
    /// Wrap `client`.
    #[must_use]
    pub fn new(client: impl Into<String>) -> Self {
        Self(client.into())
    }

    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

/// The secret of the Tailscale OAuth client. Its `Debug` hides the value and it has no `Display`.
#[derive(Clone, PartialEq, Eq)]
pub struct ClientSecret(String);

impl ClientSecret {
    /// Wrap the already-decrypted `secret`.
    #[must_use]
    pub fn new(secret: impl Into<String>) -> Self {
        Self(secret.into())
    }

    pub(super) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ClientSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ClientSecret(<redacted>)")
    }
}

/// An OAuth client id and its secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthCredentials {
    client: ClientId,
    secret: ClientSecret,
}

impl OAuthCredentials {
    /// The `client` id with its `secret`.
    #[must_use]
    pub const fn new(client: ClientId, secret: ClientSecret) -> Self {
        Self { client, secret }
    }

    pub(super) const fn client(&self) -> &ClientId {
        &self.client
    }

    pub(super) const fn secret(&self) -> &ClientSecret {
        &self.secret
    }
}
