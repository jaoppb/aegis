//! The Tailscale OAuth client of the controller, read from the environment of `aegis bootstrap`
//! and never from its arguments, so it shows up in no process list.

use std::fmt;

use aegis_domain::Secret;

use crate::tailscale::{ClientId, ClientSecret, OAuthCredentials};

const CLIENT_ID_VAR: &str = "TS_OAUTH_CLIENT_ID";
const CLIENT_SECRET_VAR: &str = "TS_OAUTH_CLIENT_SECRET";

/// The OAuth credentials are missing or cannot be written to a dotenv file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialsError {
    /// The variable is not set.
    Missing(&'static str),
    /// The variable holds a character outside letters, digits, `-` and `_`.
    Unsupported(&'static str),
}

impl fmt::Display for CredentialsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(name) => write!(formatter, "{name} is not set"),
            Self::Unsupported(name) => write!(
                formatter,
                "{name} may hold only letters, digits, `-` and `_`"
            ),
        }
    }
}

impl std::error::Error for CredentialsError {}

/// The credentials as the minter takes them and as the controller's dotenv file holds them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthEnvironment {
    credentials: OAuthCredentials,
    dotenv: Secret,
}

fn value(name: &'static str, found: Option<String>) -> Result<String, CredentialsError> {
    let text = found.ok_or(CredentialsError::Missing(name))?;
    let plain =
        |candidate: char| candidate.is_ascii_alphanumeric() || matches!(candidate, '-' | '_');
    let valid = !text.is_empty() && text.chars().all(plain);
    valid
        .then_some(text)
        .ok_or(CredentialsError::Unsupported(name))
}

impl OAuthEnvironment {
    /// Read `TS_OAUTH_CLIENT_ID` and `TS_OAUTH_CLIENT_SECRET`.
    ///
    /// # Errors
    ///
    /// Returns a [`CredentialsError`] naming the variable that is missing or unusable. The
    /// error never holds a value.
    pub fn from_environment() -> Result<Self, CredentialsError> {
        let client = std::env::var(CLIENT_ID_VAR).ok();
        let secret = std::env::var(CLIENT_SECRET_VAR).ok();
        Self::from_values(client, secret)
    }

    /// The credentials from the two values, each `None` when it is not set.
    ///
    /// # Errors
    ///
    /// Returns a [`CredentialsError`] naming the value that is missing or unusable. The error
    /// never holds a value.
    pub fn from_values(
        client: Option<String>,
        secret: Option<String>,
    ) -> Result<Self, CredentialsError> {
        let client = value(CLIENT_ID_VAR, client)?;
        let secret = value(CLIENT_SECRET_VAR, secret)?;
        let dotenv = Secret::new(format!(
            "{CLIENT_ID_VAR}={client}\n{CLIENT_SECRET_VAR}={secret}\n"
        ));
        let credentials = OAuthCredentials::new(ClientId::new(client), ClientSecret::new(secret));
        Ok(Self {
            credentials,
            dotenv,
        })
    }

    /// The credentials for the Tailscale API.
    #[must_use]
    pub const fn credentials(&self) -> &OAuthCredentials {
        &self.credentials
    }

    /// The dotenv text the controller reads, to be encrypted before it is written.
    #[must_use]
    pub const fn dotenv(&self) -> &Secret {
        &self.dotenv
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn given(client: &str, secret: &str) -> Result<OAuthEnvironment, CredentialsError> {
        OAuthEnvironment::from_values(Some(client.to_owned()), Some(secret.to_owned()))
    }

    #[test]
    fn the_dotenv_holds_both_values() {
        let environment = given("abc", "tskey-client-x_y").unwrap();
        let text = environment.dotenv().expose();
        assert_eq!(
            text,
            "TS_OAUTH_CLIENT_ID=abc\nTS_OAUTH_CLIENT_SECRET=tskey-client-x_y\n"
        );
    }

    #[test]
    fn a_missing_value_is_named_without_printing_the_other() {
        let error = OAuthEnvironment::from_values(Some("abc".to_owned()), None).unwrap_err();
        assert_eq!(error, CredentialsError::Missing("TS_OAUTH_CLIENT_SECRET"));
    }

    #[test]
    fn a_value_that_could_inject_a_line_or_interpolate_is_refused() {
        let newline = given("abc", "x\nEVIL=1").unwrap_err();
        assert_eq!(
            newline,
            CredentialsError::Unsupported("TS_OAUTH_CLIENT_SECRET")
        );
        let dollar = given("a$b", "ok").unwrap_err();
        assert_eq!(dollar, CredentialsError::Unsupported("TS_OAUTH_CLIENT_ID"));
        assert!(given("", "ok").is_err());
    }

    #[test]
    fn the_secret_never_appears_in_debug_output() {
        let environment = given("abc", "supersecretvalue").unwrap();
        assert!(!format!("{environment:?}").contains("supersecretvalue"));
    }
}
