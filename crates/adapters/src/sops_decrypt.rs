//! The `sops` CLI adapter of the [`SecretDecryptor`] port.
//!
//! The child gets a cleared environment with only the variables that find the host's own age key,
//! and nothing it prints is ever kept: a failure names the file and the reason (an exit status, a
//! launch error or the refusal of a foreign key group), never the output.

use std::process::Output;
use std::time::Duration;

use aegis_application::{DecryptError, Reason, SecretDecryptor};
use aegis_domain::{HostPath, Secret};
use serde_norway::Value;
use tokio::process::Command;

use crate::cli;
use crate::sops::Layout;

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

/// The key groups `sops` can use besides age, which older `sops` wrote as empty `null` entries.
/// The file's author picks the groups, and `sops` would act on them (a request to a URL of the
/// author's choosing, `gpg`, cloud credentials of the host). Only these names are tolerated when
/// empty; every other name, and every non-empty group, is refused, so a group a newer `sops` adds
/// (such as `hckms`) is refused too.
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
    let age_only = |group: &Value| {
        let keys = group
            .as_mapping()
            .map(|map| map.keys().all(|key| key.as_str() == Some("age")));
        keys.unwrap_or(false)
    };
    groups
        .as_sequence()
        .is_some_and(|list| list.iter().all(age_only))
}

fn metadata_entry_is_age_only(key: &Value, value: &Value) -> bool {
    match key.as_str() {
        Some("key_groups") => groups_are_age_only(value),
        Some(name) if METADATA_KEYS.contains(&name) => true,
        Some(name) => FOREIGN_KEYS.contains(&name) && is_empty_value(value),
        None => false,
    }
}

/// Whether a top-level key could be read as, or merged into, the `sops` metadata by the parser in
/// `sops` without being exactly `sops`: another case, a merge key, or any non-ASCII name that
/// Unicode folding could turn into it.
fn masquerades_as_metadata(key: &str) -> bool {
    (key.eq_ignore_ascii_case("sops") && key != "sops") || key == "<<" || !key.is_ascii()
}

/// YAML tags (`!!binary sops`) and directives (`%TAG`) that change how the parser in `sops` reads
/// a key, but that this parser drops silently. `sops` writes none, and its encrypted values and
/// comments are `ENC[...]`; a plaintext part of a file, such as a key name or a value left
/// unencrypted by an `unencrypted_*` option, must avoid `!` and a leading `%` too.
fn has_tag_or_directive(text: &str) -> bool {
    text.contains('!') || text.lines().any(|line| line.starts_with('%'))
}

/// YAML and JSON: one document whose `sops` metadata holds only keys that are known and age-only.
/// A file the parser cannot read, several documents, duplicate keys, a tag or directive, and a
/// merge key in the metadata are all refused, because the parser in `sops` may read them
/// differently.
fn structured_is_age_only(text: &str) -> bool {
    let Ok(document) = serde_norway::from_str::<Value>(text) else {
        return false;
    };
    let Some(root) = document.as_mapping() else {
        return false;
    };
    if has_tag_or_directive(text)
        || root
            .keys()
            .filter_map(Value::as_str)
            .any(masquerades_as_metadata)
    {
        return false;
    }
    let metadata = root.get("sops").and_then(Value::as_mapping);
    metadata.is_some_and(|sops| {
        sops.iter()
            .all(|(key, value)| metadata_entry_is_age_only(key, value))
    })
}

/// Whether the flattened metadata key `key` (lowercase, without any `sops_` prefix) is one that
/// `sops` writes for age: a known metadata key, or `key_groups__list_<n>__map_age__...`.
fn flat_key_is_age_only(key: &str) -> bool {
    let mut parts = key.split("__");
    match parts.next().unwrap_or_default() {
        "key_groups" => parts.nth(1) == Some("map_age"),
        name => METADATA_KEYS.contains(&name),
    }
}

/// Dotenv: every `sops_<key>=` entry must be age-only metadata. Other entries are the file's data.
fn dotenv_is_age_only(text: &str) -> bool {
    let entries = text.lines().filter_map(|line| line.split_once('='));
    let keys = entries.map(|(key, _)| key.trim().to_ascii_lowercase());
    let mut metadata = keys.filter_map(|key| key.strip_prefix("sops_").map(str::to_owned));
    metadata.all(|key| flat_key_is_age_only(&key))
}

/// Where an INI scan is: outside or inside a `[sops]` section, or already refused.
#[derive(Clone, Copy, PartialEq, Eq)]
enum IniScan {
    Outside,
    Inside,
    Refused,
}

const fn section_scan(name: &str) -> IniScan {
    if name.eq_ignore_ascii_case("sops") {
        IniScan::Inside
    } else {
        IniScan::Outside
    }
}

fn ini_step(scan: IniScan, line: &str) -> IniScan {
    let line = line.trim();
    if scan == IniScan::Refused {
        return scan;
    }
    if let Some(section) = line.strip_prefix('[') {
        let named = section.trim_end_matches(']').trim();
        return section_scan(named);
    }
    let key = line.split(['=', ':']).next().unwrap_or_default().trim();
    let skipped = key.is_empty() || key.starts_with([';', '#']);
    let fine =
        scan == IniScan::Outside || skipped || flat_key_is_age_only(&key.to_ascii_lowercase());
    if fine { scan } else { IniScan::Refused }
}

/// INI: every key of a `[sops]` section (in any case) must be age-only metadata.
fn ini_is_age_only(text: &str) -> bool {
    text.lines().fold(IniScan::Outside, ini_step) != IniScan::Refused
}

/// Dotenv and INI: ASCII only, because a non-ASCII key could fold into a name `sops` knows, and
/// only age metadata in the flattened `sops` entries.
fn flat_is_age_only(layout: Layout, text: &str) -> bool {
    let metadata = match layout {
        Layout::Ini => ini_is_age_only(text),
        _ => dotenv_is_age_only(text),
    };
    text.is_ascii() && metadata
}

/// Whether the file read as `layout` uses only age recipients.
fn is_age_only(layout: Layout, text: &str) -> bool {
    match layout {
        Layout::Json | Layout::Yaml => structured_is_age_only(text),
        Layout::Ini | Layout::Dotenv => flat_is_age_only(layout, text),
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

    fn command(&self, file: &HostPath, layout: Layout) -> Command {
        let format = layout.sops_type();
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
        let layout = Layout::of(file.as_str());
        let text = tokio::fs::read_to_string(file.as_str()).await;
        let text = text.map_err(|cause| failed(file, &cause.kind()))?;
        is_age_only(layout, &text)
            .then_some(())
            .ok_or_else(|| failed(file, &"only age recipients are accepted"))?;
        let output = cli::run(self.command(file, layout), String::new(), SOPS_TIMEOUT)
            .await
            .map_err(|reason| failed(file, &reason))?;
        plaintext(file, &output)
    }
}
