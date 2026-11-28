//! Recognises SOPS-encrypted files by their metadata. It checks the format and never decrypts.

use aegis_application::EncryptionCheck;
use aegis_domain::RepoPath;
use serde_norway::Value;

/// A file whose extension says how its SOPS metadata is laid out, and which `--input-type` and
/// `--output-type` `sops` gets for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// A JSON file.
    Json,
    /// A YAML file.
    Yaml,
    /// An INI file.
    Ini,
    /// Any other name: a dotenv file.
    Dotenv,
}

impl Layout {
    /// The layout of the file at `file`; every name that is not JSON, YAML or INI is dotenv.
    #[must_use]
    pub fn of(file: &str) -> Self {
        let extension = file.rsplit_once('.').map(|(_, extension)| extension);
        match extension {
            Some("json") => Self::Json,
            Some("yaml" | "yml") => Self::Yaml,
            Some("ini") => Self::Ini,
            _ => Self::Dotenv,
        }
    }

    /// The format name `sops` takes.
    #[must_use]
    pub const fn sops_type(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Yaml => "yaml",
            Self::Ini => "ini",
            Self::Dotenv => "dotenv",
        }
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
        match Layout::of(file.as_str()) {
            Layout::Json | Layout::Yaml => structured(text),
            Layout::Ini => ini(text),
            Layout::Dotenv => dotenv(text) || structured(text),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Layout;

    #[test]
    fn structured_extensions_have_their_layout() {
        assert_eq!(Layout::of("a.json"), Layout::Json);
        assert_eq!(Layout::of("a.yaml"), Layout::Yaml);
        assert_eq!(Layout::of("dir/a.yml"), Layout::Yaml);
    }

    #[test]
    fn the_other_extensions_are_ini_or_dotenv() {
        assert_eq!(Layout::of("a.ini"), Layout::Ini);
        assert_eq!(Layout::of("a.env"), Layout::Dotenv);
        assert_eq!(Layout::of(".env"), Layout::Dotenv);
        assert_eq!(Layout::of("secrets"), Layout::Dotenv);
        assert_eq!(Layout::of("a.pem"), Layout::Dotenv);
    }

    #[test]
    fn each_layout_has_its_sops_type() {
        assert_eq!(Layout::Json.sops_type(), "json");
        assert_eq!(Layout::Yaml.sops_type(), "yaml");
        assert_eq!(Layout::Ini.sops_type(), "ini");
        assert_eq!(Layout::Dotenv.sops_type(), "dotenv");
    }
}
