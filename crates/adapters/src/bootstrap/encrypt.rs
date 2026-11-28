//! The `sops` CLI adapter that encrypts the controller's credentials to the host's age key. The
//! plaintext goes to `sops` on its stdin and the ciphertext comes back on stdout: neither touches
//! the disk, and nothing `sops` prints is ever kept in an error.

use std::process::Output;
use std::time::Duration;

use aegis_application::{AgeRecipient, BootstrapError, FileText, Reason, SecretEncryptor};
use aegis_domain::Secret;
use tokio::process::Command;

use crate::cli;

/// How long one encryption may take.
const SOPS_TIMEOUT: Duration = Duration::from_mins(1);

/// The environment `sops` inherits, and nothing else.
const INHERITED: [&str; 3] = ["PATH", "HOME", "XDG_CONFIG_HOME"];

/// Encrypts with the `sops` executable of the `PATH`.
#[derive(Debug, Clone, Default)]
pub struct SopsEncrypt;

fn command(recipient: &AgeRecipient) -> Command {
    let mut command = Command::new("sops");
    command.args(["--encrypt", "--age"]).arg(recipient.as_str());
    command.args([
        "--input-type",
        "dotenv",
        "--output-type",
        "dotenv",
        "/dev/stdin",
    ]);
    command.current_dir("/");
    cli::inherit_only(&mut command, &INHERITED);
    command
}

fn ciphertext(output: &Output) -> Result<FileText, Reason> {
    if !output.status.success() {
        return Err(Reason::new(&format!("sops exited with {}", output.status)));
    }
    let text = String::from_utf8(output.stdout.clone());
    text.map(FileText::new)
        .map_err(|_| Reason::new(&"sops printed text that is not UTF-8"))
}

impl SecretEncryptor for SopsEncrypt {
    async fn encrypt(
        &self,
        plaintext: &Secret,
        recipient: &AgeRecipient,
    ) -> Result<FileText, BootstrapError> {
        let input = plaintext.expose().to_owned();
        let output = cli::run(command(recipient), input, SOPS_TIMEOUT).await;
        let output = output.map_err(BootstrapError::Encrypt)?;
        ciphertext(&output).map_err(BootstrapError::Encrypt)
    }
}
