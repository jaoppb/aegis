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
use serde_norway::Value;
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
/// file that uses any is refused.
const FOREIGN_KEYS: [&str; 5] = ["kms", "gcp_kms", "azure_kv", "hc_vault", "pgp"];

/// The keys of the `sops` metadata of a YAML or JSON file that are accepted. A foreign group is
/// accepted only when empty, as older `sops` wrote it. Anything else, such as a misspelt, escaped,
/// differently cased or Unicode-folded name that the parser in `sops` might read as a group, is
/// refused.
const METADATA_KEYS: [&str; 12] = [
    "age",
    "mac",
    "version",
    "lastmodified",
    "unencrypted_suffix",
    "encrypted_suffix",
    "unencrypted_regex",
    "encrypted_regex",
    "unencrypted_comment_regex",
    "encrypted_comment_regex",
    "mac_only_encrypted",
    "shamir_threshold",
];

fn is_empty_value(value: &Value) -> bool {
    value.is_null() || value.as_sequence().is_some_and(Vec::is_empty)
}

/// A `key_groups` list of which every group holds age recipients only.
fn groups_are_age_only(groups: &Value) -> bool {
    let listed = groups.as_sequence().into_iter().flatten();
    let mut maps = listed.map(Value::as_mapping);
    maps.all(|group| group.is_some_and(|map| map.keys().all(|key| key.as_str() == Some("age"))))
}

fn metadata_entry_is_age_only(key: &Value, value: &Value) -> bool {
    match key.as_str() {
        Some("key_groups") => groups_are_age_only(value),
        Some(name) if METADATA_KEYS.contains(&name) => true,
        Some(name) => FOREIGN_KEYS.contains(&name) && is_empty_value(value),
        None => false,
    }
}

/// Whether a top-level key could be read as the `sops` metadata by the parser in `sops` without
/// being exactly `sops`: another case, or any non-ASCII name that Unicode folding could turn into it.
fn masquerades_as_metadata(key: &str) -> bool {
    (key.eq_ignore_ascii_case("sops") && key != "sops") || !key.is_ascii()
}

/// YAML and JSON: one document whose `sops` metadata holds only keys that are known and age-only.
/// A file the parser cannot read, several documents, duplicate keys and unresolved merge keys are
/// all refused, because the parser in `sops` may read them differently.
fn structured_is_age_only(text: &str) -> bool {
    let Ok(document) = serde_norway::from_str::<Value>(text) else {
        return false;
    };
    let Some(root) = document.as_mapping() else {
        return false;
    };
    let mut names = root.keys().filter_map(Value::as_str);
    let metadata = root.get("sops").and_then(Value::as_mapping);
    let clean = !names.any(masquerades_as_metadata);
    clean
        && metadata.is_some_and(|sops| {
            sops.iter()
                .all(|(key, value)| metadata_entry_is_age_only(key, value))
        })
}

/// Dotenv and INI: the flattened `sops_<group>__...` and `<group>__...` entries, and any
/// non-ASCII key (it could fold into one).
fn flat_is_age_only(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let keys = text.lines().filter_map(|line| line.split_once('='));
    let ascii = keys.map(|(key, _)| key).all(str::is_ascii);
    ascii
        && !FOREIGN_KEYS
            .iter()
            .any(|group| lower.contains(&format!("{group}__")))
}

/// Whether the file at `file`, read as its extension says, uses only age recipients.
fn is_age_only(file: &Path, text: &str) -> bool {
    match format_of(file) {
        "json" | "yaml" => structured_is_age_only(text),
        _ => flat_is_age_only(text),
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
        let text = tokio::fs::read_to_string(file.as_str()).await;
        let text = text.map_err(|cause| failed(file, &cause.kind()))?;
        is_age_only(Path::new(file.as_str()), &text)
            .then_some(())
            .ok_or_else(|| failed(file, &"only age recipients are accepted"))?;
        let output = cli::run(self.command(file), String::new(), SOPS_TIMEOUT)
            .await
            .map_err(|reason| failed(file, &reason))?;
        plaintext(file, &output)
    }
}
