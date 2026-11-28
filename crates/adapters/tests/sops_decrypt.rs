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

#[test]
fn sops_only_inherits_the_variables_that_find_the_age_key() {
    runtime().block_on(async {
        let sandbox = Sandbox::new("env", 0).await;
        let file = sandbox.encrypted("db.env").await;
        sandbox.decryptor().decrypt(&file).await.unwrap();
        let log = tokio::fs::read_to_string(sandbox.log()).await.unwrap();
        let names = log.lines().last().unwrap();
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
