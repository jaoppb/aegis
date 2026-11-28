//! The `sops` CLI adapter of the [`SecretDecryptor`] port.
//!
//! The child gets a cleared environment with only the variables that find the host's own age key,
//! and nothing it prints is ever kept: a failure names the file and the reason (an exit status, a
//! launch error or the refusal of a foreign key group), never the output.

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

/// The key groups `sops` can use besides age. The file's author picks them, and `sops` would act on
/// them (a request to a URL of the author's choosing, `gpg`, cloud credentials of the host), so a
/// file that names any is refused.
const FOREIGN_KEYS: [&str; 5] = ["kms", "gcp_kms", "azure_kv", "hc_vault", "pgp"];

/// Whether `rest`, the text after a group name, is a key with a value: `: <value>` or `": <value>`,
/// not the empty `null`, `[]` or `~` that older `sops` wrote for a group the file does not use.
fn is_key_with_value(rest: &str) -> bool {
    let after = rest.trim_start_matches(['"', '\'']).trim_start();
    let Some(value) = after.strip_prefix(':') else {
        return false;
    };
    let value = value.trim_start();
    !["null", "[]", "~"]
        .iter()
        .any(|empty| value.starts_with(empty))
}

/// Whether `text` names a key group other than age, in any format. It reads tokens, not a
/// structure, so a parser that differs from the one in `sops` (case, several YAML documents, merge
/// keys, duplicate keys) cannot hide one. The flattened dotenv and INI forms (`sops_kms__...`,
/// `kms__...`), and the groups nested under `key_groups`, are found the same way.
fn has_foreign_key(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    FOREIGN_KEYS.iter().any(|group| {
        let flattened = lower.contains(&format!("{group}__"));
        flattened || lower.split(group).skip(1).any(is_key_with_value)
    })
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
        let text = tokio::fs::read_to_string(file.as_str()).await;
        let text = text.map_err(|cause| failed(file, &cause.kind()))?;
        (!has_foreign_key(&text))
            .then_some(())
            .ok_or_else(|| failed(file, &"only age recipients are accepted"))?;
        let output = cli::run(self.command(file), String::new(), SOPS_TIMEOUT)
            .await
            .map_err(|reason| failed(file, &reason))?;
        plaintext(file, &output)
    }
}
