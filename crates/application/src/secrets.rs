//! The ports that decrypt a stack's secrets and keep the plaintext on a volatile store for the
//! length of a deploy. The plaintext never reaches a log, an error or persistent disk.

use std::fmt;
use std::future::Future;

use aegis_domain::{HostPath, ProjectName, Secret, SlotName};

use crate::source::Reason;

/// Why a file could not be decrypted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecryptError {
    /// The file does not exist.
    Missing,
    /// The file exists but could not be decrypted, for instance because it is not encrypted to
    /// this host's key. The reason never holds the decryption tool's output.
    Failed(Reason),
}

impl fmt::Display for DecryptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => formatter.write_str("the file is missing"),
            Self::Failed(reason) => write!(formatter, "decryption failed: {reason}"),
        }
    }
}

impl std::error::Error for DecryptError {}

/// Decrypts a file with the host's own key.
pub trait SecretDecryptor {
    /// The plaintext of the encrypted file at `file`.
    ///
    /// # Errors
    ///
    /// Returns [`DecryptError::Missing`] when there is no such file and
    /// [`DecryptError::Failed`] when it cannot be decrypted.
    fn decrypt(&self, file: &HostPath)
    -> impl Future<Output = Result<Secret, DecryptError>> + Send;
}

/// Where a placed secret goes: a named file of one project's workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    project: ProjectName,
    name: SlotName,
}

impl Slot {
    /// The file `name` of the workspace of `project`.
    #[must_use]
    pub const fn new(project: ProjectName, name: SlotName) -> Self {
        Self { project, name }
    }

    /// The project whose workspace holds the file.
    #[must_use]
    pub const fn project(&self) -> &ProjectName {
        &self.project
    }

    /// The file name inside the workspace.
    #[must_use]
    pub const fn name(&self) -> &SlotName {
        &self.name
    }
}

/// Keeping or removing the plaintext failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreError(Reason);

impl StoreError {
    /// A failure for `reason`.
    #[must_use]
    pub const fn new(reason: Reason) -> Self {
        Self(reason)
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "secret store failed: {}", self.0)
    }
}

impl std::error::Error for StoreError {}

/// A volatile, private place for decrypted files. An implementation refuses to keep a file
/// anywhere that survives a reboot.
pub trait SecretStore {
    /// Write `content` to `slot` and give back its absolute path.
    ///
    /// # Errors
    ///
    /// Returns a [`StoreError`] when the store is not volatile or the file cannot be written.
    fn place(
        &self,
        slot: &Slot,
        content: &Secret,
    ) -> impl Future<Output = Result<HostPath, StoreError>> + Send;

    /// Remove everything placed for `project`. Removing nothing is not an error.
    ///
    /// # Errors
    ///
    /// Returns a [`StoreError`] when plaintext may remain.
    fn discard(&self, project: &ProjectName)
    -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Remove everything left by a previous run, such as after a crash.
    ///
    /// # Errors
    ///
    /// Returns a [`StoreError`] when plaintext may remain.
    fn sweep(&self) -> impl Future<Output = Result<(), StoreError>> + Send;
}
