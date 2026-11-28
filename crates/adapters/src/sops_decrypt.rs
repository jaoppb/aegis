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

/// Whether `rest`, the flattened key after `key_groups__list_`, is `<digits>__map_age` followed by
/// nothing or by `__list_...`: a key group whose only key is `age`. `sops` splits a flattened key
/// on `__map_` and `__list_` only, so a looser reading of `__` would let a group it builds from
/// `key_groups__map_hc_vault__map_age` pass.
fn key_group_is_age_only(rest: &str) -> bool {
    let Some((index, tail)) = rest.split_once("__map_age") else {
        return false;
    };
    let numbered = !index.is_empty() && index.chars().all(|digit| digit.is_ascii_digit());
    numbered && (tail.is_empty() || tail.starts_with("__list_"))
}

/// Whether the first token of the flattened key `key`, up to the first `__map_` or `__list_`, is a
/// metadata name `sops` writes for age.
fn first_token_is_metadata(key: &str) -> bool {
    let first = key.split("__map_").next().unwrap_or_default();
    let first = first.split("__list_").next().unwrap_or_default();
    METADATA_KEYS.contains(&first)
}

/// Whether the flattened metadata key `key` (lowercase, without any `sops_` prefix) is one that
/// `sops` writes for age: a known metadata name as its first token, or a `key_groups` entry whose
/// group holds only `age`.
fn flat_key_is_age_only(key: &str) -> bool {
    let group = key.strip_prefix("key_groups__list_");
    group.map_or_else(|| first_token_is_metadata(key), key_group_is_age_only)
}

/// Dotenv: every `sops_<key>=` entry must be age-only metadata. Other entries are the file's data.
fn dotenv_is_age_only(text: &str) -> bool {
    let entries = text.lines().filter_map(|line| line.split_once('='));
    let keys = entries.map(|(key, _)| key.trim().to_ascii_lowercase());
    let mut metadata = keys.filter_map(|key| key.strip_prefix("sops_").map(str::to_owned));
    metadata.all(|key| flat_key_is_age_only(&key))
}

/// Whether an INI line is one physical line for every reader. The INI reader in `sops` joins a line
/// that ends with `\` to the next one and reads `"""`, `'''` and backtick values over several lines,
/// so a `[section]` or a key can hide inside a value for a line-based scan. `sops` writes none of
/// these (its values are `ENC[...]`), so a file with any is refused.
fn is_plain_ini_line(line: &str) -> bool {
    !line.ends_with('\\')
        && !["\"\"\"", "'''", "`"]
            .iter()
            .any(|quote| line.contains(quote))
}

/// The name of a section header that is exactly `[name]`, which is how every INI reader takes it.
/// A header followed by anything (`[sops] ; c`, `[sops]x`) is read differently by different
/// readers, so it has no name here.
fn section_name(line: &str) -> Option<&str> {
    let inner = line.strip_prefix('[').unwrap_or(line);
    let name = inner.strip_suffix(']')?;
    (!name.contains(']')).then_some(name.trim())
}

/// Which INI section a scan is in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Sops,
    Other,
}

const fn section_named(name: &str) -> Section {
    if name.eq_ignore_ascii_case("sops") {
        Section::Sops
    } else {
        Section::Other
    }
}

/// One INI line, in `section`. `None` refuses the file: a line that is not plain, a header that is
/// not exactly `[name]`, or a key of the `[sops]` section that is not age metadata.
fn ini_step(section: Section, line: &str) -> Option<Section> {
    let line = line.trim();
    if !is_plain_ini_line(line) {
        return None;
    }
    if line.starts_with('[') {
        return section_name(line).map(section_named);
    }
    let key = line.split(['=', ':']).next().unwrap_or_default().trim();
    let skipped = key.is_empty() || key.starts_with([';', '#']);
    let fine =
        section == Section::Other || skipped || flat_key_is_age_only(&key.to_ascii_lowercase());
    fine.then_some(section)
}

/// INI: every key of a `[sops]` section (in any case) must be age-only metadata, and no line may
/// be one that readers could split or join differently.
fn ini_is_age_only(text: &str) -> bool {
    text.lines().try_fold(Section::Other, ini_step).is_some()
}

/// Whether the file read as `layout` uses only age recipients. Dotenv and INI files must also be
/// ASCII, because a non-ASCII key could fold into a name `sops` knows.
fn is_age_only(layout: Layout, text: &str) -> bool {
    match layout {
        Layout::Json | Layout::Yaml => structured_is_age_only(text),
        Layout::Ini => text.is_ascii() && ini_is_age_only(text),
        Layout::Dotenv => text.is_ascii() && dotenv_is_age_only(text),
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
