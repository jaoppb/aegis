//! Recognises SOPS-encrypted files by their metadata. It checks the format and never decrypts.

use aegis_application::EncryptionCheck;
use aegis_domain::RepoPath;
use serde_norway::Value;

/// A file whose extension says how its SOPS metadata is laid out.
enum Layout {
    Structured,
    Ini,
    Dotenv,
}

fn layout(file: &RepoPath) -> Layout {
    let extension = file
        .as_str()
        .rsplit_once('.')
        .map(|(_, extension)| extension);
    match extension {
        Some("json" | "yaml" | "yml") => Layout::Structured,
        Some("ini") => Layout::Ini,
        _ => Layout::Dotenv,
    }
}

fn has_key(mapping: &Value, key: &str) -> bool {
    mapping.get(key).is_some()
}

/// YAML and JSON: a top-level `sops` mapping holding the `mac` and `version`.
fn structured(text: &str) -> bool {
    let Ok(document) = serde_norway::from_str::<Value>(text) else {
        return false;
    };
    document
        .get("sops")
        .is_some_and(|sops| has_key(sops, "mac") && has_key(sops, "version"))
}

fn has_line(text: &str, prefix: &str) -> bool {
    text.lines()
        .any(|line| line.trim_start().starts_with(prefix))
}

/// INI: a `[sops]` section that holds the `mac` and the `version`.
fn ini(text: &str) -> bool {
    let section = text
        .lines()
        .map(str::trim)
        .skip_while(|line| *line != "[sops]")
        .skip(1)
        .take_while(|line| !line.starts_with('['));
    let keys: Vec<&str> = section
        .filter_map(|line| line.split_once('='))
        .map(|(key, _)| key.trim())
        .collect();
    keys.contains(&"mac") && keys.contains(&"version")
}

/// Dotenv: `sops_mac=` and `sops_version=` entries.
fn dotenv(text: &str) -> bool {
    has_line(text, "sops_mac=") && has_line(text, "sops_version=")
}

/// The [`EncryptionCheck`] for SOPS files.
#[derive(Debug, Clone, Copy, Default)]
pub struct SopsCheck;

impl EncryptionCheck for SopsCheck {
    fn is_encrypted(&self, file: &RepoPath, text: &str) -> bool {
        match layout(file) {
            Layout::Structured => structured(text),
            Layout::Ini => ini(text),
            Layout::Dotenv => dotenv(text) || structured(text),
        }
    }
}
