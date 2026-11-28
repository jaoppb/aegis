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
        tokio::fs::write(&path, "sealed").await.unwrap();
        HostPath::new(path.display().to_string()).unwrap()
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
