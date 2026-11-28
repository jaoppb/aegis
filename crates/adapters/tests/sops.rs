//! The SOPS format check: it recognises metadata and never decrypts.

use aegis_adapters::SopsCheck;
use aegis_application::EncryptionCheck;
use aegis_domain::RepoPath;

fn encrypted(file: &str, text: &str) -> bool {
    RepoPath::new(file).is_ok_and(|path| SopsCheck.is_encrypted(&path, text))
}

#[test]
fn dotenv_needs_the_mac_and_the_version() {
    let sealed = "A=ENC[x]\nsops_mac=ENC[y]\nsops_version=3.9.0\n";
    assert!(encrypted(".env", sealed));
    assert!(encrypted("db.env", sealed));
    assert!(!encrypted(".env", "TOKEN=hunter2\n"));
    assert!(!encrypted(".env", "sops_mac=ENC[y]\n"));
}

#[test]
fn yaml_and_json_need_a_sops_mapping() {
    let yaml = "a: ENC[x]\nsops:\n  mac: m\n  version: 3.9.0\n";
    let json = r#"{"a": "ENC[x]", "sops": {"mac": "m", "version": "3.9.0"}}"#;
    assert!(encrypted("s.yaml", yaml));
    assert!(encrypted("s.yml", yaml));
    assert!(encrypted("s.json", json));
    assert!(!encrypted("s.yaml", "a: plain\n"));
    assert!(!encrypted("s.yaml", "sops: plain\n"));
    assert!(!encrypted("s.json", "not json {"));
}

#[test]
fn ini_needs_a_sops_section() {
    assert!(encrypted(
        "s.ini",
        "[a]\nk=ENC[x]\n[sops]\nmac = m\nversion = 3\n"
    ));
    assert!(!encrypted("s.ini", "[a]\nk=v\n"));
}

#[test]
fn a_file_without_an_extension_may_be_dotenv_or_structured() {
    assert!(encrypted("secret", "sops_mac=m\nsops_version=3\n"));
    assert!(encrypted("secret", "sops:\n  mac: m\n  version: 3\n"));
    assert!(!encrypted("secret", "hunter2\n"));
}

#[test]
fn partial_metadata_is_not_enough() {
    assert!(!encrypted("s.yaml", "sops:\n  mac: m\n"));
    assert!(!encrypted("s.yaml", "sops:\n  version: 3\n"));
    assert!(!encrypted("s.json", r#"{"sops": {"mac": "m"}}"#));
    assert!(!encrypted(".env", "sops_version=3\n"));
    assert!(!encrypted("s.ini", "[sops]\nversion = 3\n"));
    assert!(!encrypted("s.ini", "[sops]\nmachine = m\nversion = 3\n"));
}
