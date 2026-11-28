//! The `sops` CLI adapter against a fake `sops` executable that records how it was run.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use aegis_adapters::SopsDecrypt;
use aegis_application::{DecryptError, SecretDecryptor};
use aegis_domain::HostPath;

#[cfg(test)]
struct Sandbox {
    root: PathBuf,
}

#[cfg(test)]
impl Sandbox {
    async fn new(name: &str, exit_code: i32) -> Self {
        let root = std::env::temp_dir().join(format!("aegis-sops-{}-{name}", std::process::id()));
        tokio::fs::remove_dir_all(&root).await.ok();
        tokio::fs::create_dir_all(&root).await.unwrap();
        let sandbox = Self { root };
        let script = format!(
            "#!/bin/sh\necho \"ARGS:$*\" >> {log}\nenv | cut -d= -f1 | sort | tr '\\n' ' ' >> {log}\n\
             echo \"leaked-ciphertext-detail\" >&2\nif [ {exit_code} -eq 0 ]; then echo 'A=plain'; fi\nexit {exit_code}\n",
            log = sandbox.log().display()
        );
        tokio::fs::write(sandbox.fake(), script).await.unwrap();
        let executable = std::fs::Permissions::from_mode(0o755);
        tokio::fs::set_permissions(sandbox.fake(), executable)
            .await
            .unwrap();
        sandbox
    }

    fn log(&self) -> PathBuf {
        self.root.join("log")
    }

    fn fake(&self) -> PathBuf {
        self.root.join("fake-sops")
    }

    fn decryptor(&self) -> SopsDecrypt {
        SopsDecrypt::with_program(self.fake().display().to_string())
    }

    async fn encrypted(&self, name: &str) -> HostPath {
        let path = self.root.join(name);
        let text = sealed_text(&path);
        tokio::fs::write(&path, text).await.unwrap();
        HostPath::new(path.display().to_string()).unwrap()
    }
}

/// Metadata that passes the age-only check, in the format the file name asks for.
#[cfg(test)]
fn sealed_text(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("yaml" | "yml") => "a: ENC[x]\nsops:\n  age: []\n  mac: m\n  version: 3.13.3\n",
        Some("json") => r#"{"a": "ENC[x]", "sops": {"age": [], "mac": "m", "version": "3"}}"#,
        Some("ini") => "[a]\nx = ENC[x]\n\n[sops]\nmac = m\nversion = 3\n",
        _ => "A=ENC[x]\nsops_mac=m\n",
    }
}

#[cfg(test)]
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn a_file_is_decrypted_with_its_own_format_and_nothing_is_kept_on_disk_by_the_adapter() {
    runtime().block_on(async {
        let sandbox = Sandbox::new("ok", 0).await;
        let file = sandbox.encrypted("db.env").await;
        let secret = sandbox.decryptor().decrypt(&file).await.unwrap();
        assert_eq!(secret.expose(), "A=plain\n");
        let log = tokio::fs::read_to_string(sandbox.log()).await.unwrap();
        let expected = format!("ARGS:decrypt --input-type dotenv --output-type dotenv {file}");
        assert!(log.contains(&expected), "{log}");
        let yaml = sandbox.encrypted("s.yaml").await;
        sandbox.decryptor().decrypt(&yaml).await.unwrap();
        let log = tokio::fs::read_to_string(sandbox.log()).await.unwrap();
        assert!(
            log.contains("--input-type yaml --output-type yaml"),
            "{log}"
        );
    });
}

#[cfg(test)]
async fn names_seen_by_sops() -> String {
    let sandbox = Sandbox::new("env", 0).await;
    let file = sandbox.encrypted("db.env").await;
    sandbox.decryptor().decrypt(&file).await.unwrap();
    let log = tokio::fs::read_to_string(sandbox.log()).await.unwrap();
    log.lines().last().unwrap().to_owned()
}

#[test]
fn sops_only_inherits_the_variables_that_find_the_age_key() {
    runtime().block_on(async {
        let names = names_seen_by_sops().await;
        let allowed = [
            "PATH",
            "HOME",
            "XDG_CONFIG_HOME",
            "XDG_RUNTIME_DIR",
            "SOPS_AGE_KEY_FILE",
            "SOPS_AGE_KEY_CMD",
            "SOPS_AGE_SSH_PRIVATE_KEY_FILE",
            "PWD",
            "SHLVL",
            "_",
        ];
        assert!(
            names.split_whitespace().any(|name| name == "PATH"),
            "{names}"
        );
        let stray: Vec<&str> = names
            .split_whitespace()
            .filter(|name| !allowed.contains(name))
            .collect();
        assert!(stray.is_empty(), "{stray:?} reached sops");
    });
}

#[test]
fn a_failure_names_the_file_and_never_the_tool_output() {
    runtime().block_on(async {
        let sandbox = Sandbox::new("fail", 128).await;
        let file = sandbox.encrypted("db.env").await;
        let error = sandbox.decryptor().decrypt(&file).await.unwrap_err();
        let message = error.to_string();
        assert!(matches!(error, DecryptError::Failed(_)));
        assert!(message.contains("db.env"), "{message}");
        assert!(message.contains("128"), "{message}");
        assert!(!message.contains("leaked-ciphertext-detail"), "{message}");
    });
}

#[test]
fn a_missing_file_is_reported_without_running_sops() {
    runtime().block_on(async {
        let sandbox = Sandbox::new("missing", 0).await;
        let file = HostPath::new(sandbox.root.join("absent.env").display().to_string()).unwrap();
        let error = sandbox.decryptor().decrypt(&file).await.unwrap_err();
        assert_eq!(error, DecryptError::Missing);
        assert!(!sandbox.log().exists());
    });
}

#[cfg(test)]
async fn refused(name: &str, text: &str) -> (String, bool) {
    let sandbox = Sandbox::new(name, 0).await;
    let path = sandbox.root.join(name);
    tokio::fs::write(&path, text).await.unwrap();
    let file = HostPath::new(path.display().to_string()).unwrap();
    let error = sandbox.decryptor().decrypt(&file).await.unwrap_err();
    let ran = tokio::fs::try_exists(sandbox.log()).await.unwrap();
    (error.to_string(), ran)
}

#[cfg(test)]
async fn assert_refused(name: &str, text: &str) {
    let (message, ran) = refused(name, text).await;
    assert!(message.contains("only age recipients"), "{name}: {message}");
    assert!(!ran, "{name} reached sops");
}

#[test]
fn a_file_that_names_a_key_group_other_than_age_is_refused_without_running_sops() {
    runtime().block_on(async {
        let yaml = "a: ENC[x]\nsops:\n  hc_vault:\n    - vault_address: http://evil\n  age: []\n";
        let json = r#"{"sops": {"kms": [{"arn": "arn:x"}], "age": []}}"#;
        let dotenv = "A=ENC[x]\nsops_pgp__list_0__map_fp=ABCD\nsops_mac=m\n";
        let ini = "[a]\nx=1\n[sops]\ngcp_kms__list_0__map_resource_id = r\nmac = m\n";
        assert_refused("s.yaml", yaml).await;
        assert_refused("s.json", json).await;
        assert_refused("s.env", dotenv).await;
        assert_refused("s.ini", ini).await;
    });
}

#[test]
fn the_check_cannot_be_hidden_by_case_documents_merge_keys_duplicates_or_key_groups() {
    runtime().block_on(async {
        assert_refused(
            "a.json",
            r#"{"sops": {"HC_VAULT": [{"vault_address": "x"}]}}"#,
        )
        .await;
        assert_refused("b.yaml", "sops: {Kms: [{arn: x}]}\n---\nb: 1\n").await;
        assert_refused(
            "c.yaml",
            "sops:\n  <<: *g\n  azure_kv:\n    - vault_url: x\n",
        )
        .await;
        assert_refused("d.json", r#"{"sops": {"pgp": null, "pgp": [{"fp": "A"}]}}"#).await;
        let nested =
            "sops:\n  key_groups:\n    - age: []\n      hc_vault:\n        - vault_address: x\n";
        assert_refused("e.yaml", nested).await;
        assert_refused(
            "f.env",
            "A=1\nsops_key_groups__list_0__map_hc_vault__list_0__map_vault_address=x\n",
        )
        .await;
    });
}

#[test]
fn escapes_aliases_explicit_keys_and_unicode_folding_cannot_hide_a_group() {
    runtime().block_on(async {
        assert_refused("a.yaml", "sops:\n  \"\\x6bms\": [{arn: x}]\n").await;
        assert_refused("b.json", r#"{"sops": {"k\u006dms": [{"arn": "x"}]}}"#).await;
        assert_refused("c.json", "{\"sops\": {\"\u{212a}ms\": [{\"arn\": \"x\"}]}}").await;
        assert_refused("d.json", "{\"sops\": {\"km\u{17f}\": [{\"arn\": \"x\"}]}}").await;
        assert_refused("e.yaml", "x: &k kms\nsops:\n  *k : [{arn: x}]\n").await;
        assert_refused("f.yaml", "sops:\n  ? kms # c\n  : - arn: x\n").await;
        assert_refused("g.json", r#"{"SOPS": {"age": []}}"#).await;
        assert_refused("h.json", "{\"\u{17f}ops\": {\"kms\": [{\"arn\": \"x\"}]}}").await;
        assert_refused("i.yaml", "a: 1\n").await;
    });
}

#[test]
fn a_dotenv_or_ini_file_with_anything_but_age_metadata_is_refused() {
    runtime().block_on(async {
        assert_refused(
            "a.env",
            "A=1\nsops_hckms__list_0__map_key_id=x\nsops_mac=m\n",
        )
        .await;
        assert_refused("b.env", "A=1\nSOPS_HC_VAULT__list_0__map_vault_address=x\n").await;
        assert_refused("c.env", "A=1\nsops_bogus=1\nsops_mac=m\n").await;
        let kms_group = "sops_key_groups__list_0__map_kms__list_0__map_arn=x\n";
        assert_refused("d.env", &format!("A=1\n{kms_group}")).await;
        assert_refused("e.ini", "[sops]\nhckms__list_0__map_key_id = x\nmac = m\n").await;
        assert_refused("f.ini", "[SOPS ]\nbogus = 1\n").await;
        assert_refused(
            "g.ini",
            "[a]\nx = 1\n[sops]\nmac = m\n[sops]\npgp__list_0__map_fp = A\n",
        )
        .await;
    });
}

#[test]
fn an_ini_header_with_trailing_text_or_a_joined_line_cannot_hide_a_group() {
    runtime().block_on(async {
        let group = "hc_vault__list_0__map_vault_address = http://x\n";
        assert_refused("a.ini", &format!("[sops] ; c\nmac = m\n{group}")).await;
        assert_refused("b.ini", &format!("[sops]x\nmac = m\n{group}")).await;
        assert_refused(
            "c.ini",
            &format!("[sops]\nunencrypted_suffix = _u \\\n[a]\n{group}"),
        )
        .await;
        assert_refused(
            "d.ini",
            &format!("[sops]\nunencrypted_suffix = \"\"\"x\n[a]\n\"\"\"\n{group}"),
        )
        .await;
        assert_refused("e.ini", "[sops]\nmac = `x\n[a]\n`\nhckms__x = 1\n").await;
        assert_refused("f.ini", "[a]b]\nx = 1\n").await;
        assert_refused("g.ini", "[sops]\nhckms__x = 1\n[a]\nx = 1\n").await;
        assert_refused("h.ini", "[sops]\nmac = '''x\n[a]\n'''\nhckms__x = 1\n").await;
    });
}

#[test]
fn flat_keys_are_trimmed_and_key_groups_need_an_age_group() {
    runtime().block_on(async {
        assert_refused("a.env", "A=1\n  sops_bogus=1\n").await;
        assert_refused("b.env", "A=1\nsops_bogus = 1\n").await;
        assert_refused("c.env", "A=1\nsops_key_groups=1\n").await;
        assert_refused("d.env", "A=1\nsops_key_groups__list_0=1\n").await;
        assert_refused("e.env", "A=1\nsops_key_groups__map_hc_vault__map_age=x\n").await;
        let nested = "sops_key_groups__list_0__map_age__x__map_hc_vault__list_0__map_created_at=t";
        assert_refused("f.env", &format!("A=1\n{nested}\n")).await;
        assert_refused("g.env", "A=1\nsops_mac__x=1\n").await;
        assert_refused("h.ini", "[sops]\nkey_groups__map_hc_vault__map_age = x\n").await;
        assert_refused(
            "i.ini",
            "[sops]\nkey_groups__list_x__map_age__list_0__map_enc = e\n",
        )
        .await;
        let upper = "key_groups__list_0__MAP_AGE__LIST_0__map_hc_vault__list_0__map_vault_address";
        assert_refused("j.env", &format!("A=1\nsops_{upper}=http://x\n")).await;
        assert_refused("k.ini", &format!("[sops]\n{upper} = http://x\n")).await;
        assert_refused("l.env", "A=1\nSOPS_HC_VAULT__list_0__map_x=1\n").await;
        assert_refused(
            "m.env",
            "A=1\nsops_key_groups__list___map_age__list_0__map_enc=e\n",
        )
        .await;
    });
}

#[test]
fn tags_directives_decoys_and_merge_keys_cannot_supply_the_metadata() {
    runtime().block_on(async {
        let age = r#""sops":{"age":[]}"#;
        assert_refused(
            "a.yaml",
            "!!binary sops: {age: []}\n!!binary c29wcw==: {kms: [{arn: x}]}\n",
        )
        .await;
        assert_refused(
            "b.yaml",
            "%TAG !e! tag:yaml.org,2002:\n---\nsops: {age: []}\n",
        )
        .await;
        assert_refused(
            "c.json",
            &format!("{{{age},\"SOPS\":{{\"kms\":[{{\"arn\":\"x\"}}]}}}}"),
        )
        .await;
        assert_refused(
            "d.json",
            &format!("{{{age},\"\u{17f}ops\":{{\"kms\":[{{\"arn\":\"x\"}}]}}}}"),
        )
        .await;
        assert_refused("e.json", &format!("{{\"caf\u{e9}\":1,{age}}}")).await;
        assert_refused("f.yaml", "base: &g {age: []}\n<<: *g\nsops: {age: []}\n").await;
    });
}

#[test]
fn the_allowlist_refuses_unknown_keys_bad_key_groups_and_unreadable_documents() {
    runtime().block_on(async {
        assert_refused("a.yaml", "sops: {age: [], bogus: 1}\n").await;
        assert_refused("b.yaml", "sops: {age: [], key_groups: {age: []}}\n").await;
        assert_refused("c.yaml", "sops: {age: [], key_groups: scalar}\n").await;
        assert_refused("d.yaml", "sops: {age: [], key_groups: [{kms: []}]}\n").await;
        assert_refused("e.yaml", "sops: {age: []}\n---\nb: 1\n").await;
        assert_refused("f.json", r#"{"sops":{"age":[],"age":[]}}"#).await;
        assert_refused("g.yaml", "sops:\n  base: 1\n  <<: {age: []}\n").await;
        assert_refused("h.yaml", "- sops\n").await;
        assert_refused("i.yaml", "{ not: [valid\n").await;
    });
}

#[test]
fn every_metadata_key_and_age_only_key_groups_are_accepted() {
    runtime().block_on(async {
        let all = "a: ENC[x]\nsops:\n  age: []\n  mac: m\n  version: 3.13.3\n  lastmodified: t\n  unencrypted_suffix: _u\n  encrypted_suffix: _e\n  unencrypted_regex: r\n  encrypted_regex: r\n  unencrypted_comment_regex: r\n  encrypted_comment_regex: r\n  mac_only_encrypted: true\n  shamir_threshold: 2\n  key_groups:\n    - age:\n        - recipient: age1abc\n    - age: []\n";
        accepted("all.yaml", all).await;
        accepted("comment.env", "# note\n\nA=ENC[x]\nsops_mac=m\n").await;
        let groups = "A=ENC[x]\nsops_key_groups__list_0__map_age__list_0__map_recipient=age1a\nsops_key_groups__list_1__map_age__list_0__map_enc=e\nsops_shamir_threshold=2\nsops_lastmodified=t\nsops_mac=m\n";
        accepted("groups.env", groups).await;
        accepted("empty.env", "A=ENC[x]\nsops_key_groups__list_0__map_age=\nsops_mac=m\n").await;
        accepted("map.env", "A=ENC[x]\nsops_mac__map_x=1\nsops_mac=m\n").await;
        accepted("data.env", "APP_KMS__ARN=x\nsops_mac=m\n").await;
        accepted("after.ini", "[sops]\nmac = m\n[a]\nkms__x = 1\n").await;
        accepted("quotes.ini", "[a]\np = C:\\x\nq = \"a\"\nr = it's\n[sops]\nmac = m\n").await;
        accepted("notes.ini", "[sops]\n; note\n\n# n\nmac = m\n").await;
        let ini = "[a]\nkms__x = data\n\n[sops]\nkey_groups__list_0__map_age__list_0__map_recipient = a\nshamir_threshold = 2\nmac = m\nversion = 3\n";
        accepted("groups.ini", ini).await;
    });
}

#[test]
fn non_ascii_text_or_a_colon_delimited_group_refuses_a_flat_file() {
    runtime().block_on(async {
        assert_refused("a.ini", "[sops]\n\u{212a}ms__list_0__map_arn: x\n").await;
        assert_refused("b.ini", "[sops]\nkms__list_0__map_arn: x\n").await;
        assert_refused("c.env", "A=ENC[x]\nsops_azure_\u{212a}v__x=y\n").await;
        assert_refused("d.env", "A=ENC[x]\nB=\u{e9}\nsops_mac=m\n").await;
        assert_refused("e.ini", "[a]\nx = \u{e9}\n[sops]\nmac = m\n").await;
    });
}

#[test]
fn a_yaml_directive_and_a_yaml_tag_are_each_refused_on_their_own() {
    runtime().block_on(async {
        assert_refused("a.yaml", "%YAML 1.2\n---\nsops: {age: []}\n").await;
        assert_refused("b.yaml", "a: !!str x\nsops: {age: []}\n").await;
        assert_refused("c.yaml", "sops: {age: []}\n!!binary x: 1\n").await;
    });
}

#[test]
fn a_metadata_that_is_not_a_mapping_of_names_is_refused() {
    runtime().block_on(async {
        assert_refused("a.yaml", "sops: {age: [], 1: x}\n").await;
        assert_refused("b.yaml", "sops: scalar\n").await;
        assert_refused("c.yaml", "sops: [age]\n").await;
    });
}

#[cfg(test)]
async fn assert_type(name: &str, kind: &str) {
    let sandbox = Sandbox::new(&format!("type-{kind}"), 0).await;
    let file = sandbox.encrypted(name).await;
    sandbox.decryptor().decrypt(&file).await.unwrap();
    let log = tokio::fs::read_to_string(sandbox.log()).await.unwrap();
    let expected = format!("--input-type {kind} --output-type {kind}");
    assert!(log.contains(&expected), "{name}: {log}");
}

#[test]
fn every_format_is_decrypted_with_its_own_type() {
    runtime().block_on(async {
        assert_type("a.json", "json").await;
        assert_type("a.ini", "ini").await;
        assert_type("a.yml", "yaml").await;
    });
}

#[cfg(test)]
async fn accepted(name: &str, text: &str) {
    let sandbox = Sandbox::new(name, 0).await;
    let path = sandbox.root.join(name);
    tokio::fs::write(&path, text).await.unwrap();
    let file = HostPath::new(path.display().to_string()).unwrap();
    sandbox.decryptor().decrypt(&file).await.unwrap();
    assert!(
        tokio::fs::try_exists(sandbox.log()).await.unwrap(),
        "{name}: sops did not run"
    );
}

#[test]
fn real_age_only_files_are_accepted_including_the_empty_groups_older_sops_wrote() {
    runtime().block_on(async {
        let yaml = "a: ENC[AES256_GCM,data:aA==,iv:x=,tag:y==,type:str]\nsops:\n    age:\n        - recipient: age1abc\n          enc: |\n            -----BEGIN AGE ENCRYPTED FILE-----\n    mac: ENC[AES256_GCM,data:b,iv:c,tag:d,type:str]\n    version: 3.13.3\n";
        let old = "sops:\n    kms: null\n    gcp_kms: null\n    azure_kv: []\n    hc_vault: ~\n    pgp: null\n    age:\n        - recipient: age1abc\n    version: 3.7.3\n";
        let json = r#"{"a": "ENC[x]", "sops": {"kms": null, "pgp": [], "age": [{"recipient": "age1abc"}], "version": "3.13.3"}}"#;
        let dotenv = "A=ENC[AES256_GCM,data:aA==,iv:x=,tag:y==,type:str]\nsops_age__list_0__map_recipient=age1abc\nsops_mac=ENC[x]\nsops_version=3.13.3\n";
        let ini = "[a]\nx = ENC[x]\n\n[sops]\nage__list_0__map_recipient = age1abc\nmac = ENC[x]\nversion = 3.13.3\n";
        accepted("ok.yaml", yaml).await;
        accepted("old.yaml", old).await;
        accepted("ok.json", json).await;
        accepted("ok.env", dotenv).await;
        accepted("ok.ini", ini).await;
    });
}

#[cfg(test)]
async fn refuse_every_group() {
    for group in ["kms", "gcp_kms", "azure_kv", "hc_vault", "pgp"] {
        assert_refused("g.env", &format!("A=1\nsops_{group}__list_0__map_x=y\n")).await;
        assert_refused("g.yaml", &format!("sops:\n  {group}:\n    - x: y\n")).await;
    }
}

#[test]
fn every_foreign_key_group_is_refused_in_the_yaml_and_dotenv_forms() {
    runtime().block_on(refuse_every_group());
}
