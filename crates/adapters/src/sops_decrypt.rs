//! The `sops` CLI adapter of the [`SecretDecryptor`] port.
//!
//! The child gets a cleared environment with only the variables that find the host's own age key,
//! and nothing it prints is ever kept: a failure names the file and the exit status only.

use std::path::Path;
use std::process::Output;
use std::time::Duration;

use aegis_application::{DecryptError, Reason, SecretDecryptor};
use aegis_domain::{HostPath, Secret};
use tokio::process::Command;

use crate::cli;

/// How long one decryption may take; an `age-plugin` or a key command may be slow.
const SOPS_TIMEOUT: Duration = Duration::from_mins(1);

/// The environment `sops` inherits, and nothing else. `SOPS_AGE_KEY` is left out on purpose: a key
/// held in a variable of the agent is easy to leak.
const INHERITED: [&str; 7] = [
    "PATH",
    "HOME",
    "XDG_CONFIG_HOME",
    "XDG_RUNTIME_DIR",
    "SOPS_AGE_KEY_FILE",
    "SOPS_AGE_KEY_CMD",
    "SOPS_AGE_SSH_PRIVATE_KEY_FILE",
];

/// The SOPS format of a file, from its extension, as the encryption check reads it.
fn format_of(file: &Path) -> &'static str {
    match file.extension().and_then(|extension| extension.to_str()) {
        Some("json") => "json",
        Some("yaml" | "yml") => "yaml",
        Some("ini") => "ini",
        _ => "dotenv",
    }
}

/// Runs `sops decrypt` with the age key of the agent's own host.
#[derive(Debug, Clone)]
pub struct SopsDecrypt {
    program: String,
}

impl Default for SopsDecrypt {
    fn default() -> Self {
        Self::with_program("sops")
    }
}

impl SopsDecrypt {
    /// Decrypt with the `program` executable.
    #[must_use]
    pub fn with_program<Program: Into<String>>(program: Program) -> Self {
        Self {
            program: program.into(),
        }
    }

    fn command(&self, file: &HostPath) -> Command {
        let format = format_of(Path::new(file.as_str()));
        let mut command = Command::new(&self.program);
        cli::inherit_only(&mut command, &INHERITED);
        command.args(["decrypt", "--input-type", format, "--output-type", format]);
        command.arg(file.as_str());
        command
    }
}

fn failed(file: &HostPath, cause: &impl std::fmt::Display) -> DecryptError {
    DecryptError::Failed(Reason::new(&format!(
        "sops could not decrypt {file}: {cause}"
    )))
}

fn plaintext(file: &HostPath, output: &Output) -> Result<Secret, DecryptError> {
    if !output.status.success() {
        return Err(failed(file, &output.status));
    }
    let text = std::str::from_utf8(&output.stdout).map_err(|_| failed(file, &"not UTF-8"))?;
    Ok(Secret::new(text))
}

impl SecretDecryptor for SopsDecrypt {
    async fn decrypt(&self, file: &HostPath) -> Result<Secret, DecryptError> {
        let present = tokio::fs::try_exists(file.as_str()).await;
        let present = present.map_err(|cause| failed(file, &cause.kind()))?;
        present.then_some(()).ok_or(DecryptError::Missing)?;
        let output = cli::run(self.command(file), String::new(), SOPS_TIMEOUT)
            .await
            .map_err(|reason| failed(file, &reason))?;
        plaintext(file, &output)
    }
}
