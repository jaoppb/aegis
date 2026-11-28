//! The adapters of `aegis bootstrap`.
//!
//! They cover the host's age key, the checkout and the stack files, the `sops` encryption of the
//! controller's credentials, the sidecar state check and the compose runner.

mod credentials;
mod docker;
mod encrypt;
mod host;

pub use credentials::{CredentialsError, OAuthEnvironment};
pub use docker::ComposeStacks;
pub use encrypt::SopsEncrypt;
pub use host::{Layout, LocalHost};
