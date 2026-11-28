//! The host's own files for bootstrap: the age key made by `age-keygen`, the read-only git
//! checkout and the stack files below the bootstrap directory.

use std::path::{Path, PathBuf};
use std::time::Duration;

use aegis_application::{
    AgeRecipient, BootstrapError, Existing, FileText, HostFiles, HostKey, Reason, RenderedFile,
    RepoUrl, Written,
};
use aegis_domain::HostPath;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::cli;

/// How long `age-keygen` may take.
const AGE_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a clone or a fetch may take.
const GIT_TIMEOUT: Duration = Duration::from_mins(10);

/// The environment `age-keygen` inherits, and nothing else.
const AGE_INHERITED: [&str; 2] = ["PATH", "HOME"];

/// Where the files of a bootstrap live on the host.
#[derive(Debug, Clone)]
pub struct Layout {
    checkout: PathBuf,
    stacks: PathBuf,
    key: PathBuf,
}

impl Layout {
    /// The checkout at `checkout`, the stacks rendered below `stacks` and the age key at `key`.
    #[must_use]
    pub const fn new(checkout: PathBuf, stacks: PathBuf, key: PathBuf) -> Self {
        Self {
            checkout,
            stacks,
            key,
        }
    }

    /// The age key file `sops` reads by default: `$XDG_CONFIG_HOME/sops/age/keys.txt`, else
    /// `$HOME/.config/sops/age/keys.txt`.
    ///
    /// # Errors
    ///
    /// Returns a [`Reason`] when neither variable gives an absolute directory.
    pub fn default_key() -> Result<PathBuf, Reason> {
        let configured = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
        let home = std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config"));
        let base = configured.filter(|base| base.is_absolute()).or(home);
        let base = base.filter(|base| base.is_absolute());
        let base = base.ok_or_else(|| Reason::new(&"HOME is not an absolute path"))?;
        Ok(base.join("sops").join("age").join("keys.txt"))
    }
}

/// Bootstrap's view of the local machine.
#[derive(Debug, Clone)]
pub struct LocalHost {
    layout: Layout,
}

impl LocalHost {
    /// The host laid out as `layout`.
    #[must_use]
    pub const fn new(layout: Layout) -> Self {
        Self { layout }
    }
}

fn age_command(arguments: &[&str]) -> Command {
    let mut command = Command::new("age-keygen");
    command.args(arguments);
    cli::inherit_only(&mut command, &AGE_INHERITED);
    command
}

async fn private_directory(path: &Path) -> Result<(), Reason> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let created = tokio::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)
        .await;
    created.map_err(|failure| Reason::new(&failure.kind()))
}

async fn exists(path: &Path) -> Result<bool, Reason> {
    let found = tokio::fs::try_exists(path).await;
    found.map_err(|failure| Reason::new(&failure.kind()))
}

/// Generate the age key at `path` unless a key is already there.
async fn generate(path: &Path) -> Result<(), Reason> {
    if exists(path).await? {
        return Ok(());
    }
    private_directory(path).await?;
    let command = age_command(&["-o", &path.to_string_lossy()]);
    let output = cli::run(command, String::new(), AGE_TIMEOUT).await?;
    cli::succeeded(&output, "age-keygen")
}

/// The recipient of the key at `path`, printed by `age-keygen -y`.
async fn recipient_of(path: &Path) -> Result<AgeRecipient, Reason> {
    let command = age_command(&["-y", &path.to_string_lossy()]);
    let output = cli::run(command, String::new(), AGE_TIMEOUT).await?;
    cli::succeeded(&output, "age-keygen")?;
    let printed = String::from_utf8_lossy(&output.stdout);
    AgeRecipient::new(printed.trim()).map_err(|failure| Reason::new(&failure))
}

fn git_command() -> Command {
    let mut command = Command::new("git");
    command.args(["-c", "protocol.ext.allow=never"]);
    cli::inherit_only(&mut command, &cli::GIT_INHERITED);
    command.env("GIT_TERMINAL_PROMPT", "0");
    command
}

fn clone_command(repo: &RepoUrl, directory: &Path) -> Command {
    let mut command = git_command();
    command.args(["clone", "--quiet", "--"]);
    command.arg(repo.as_str()).arg(directory);
    command
}

fn fetch_command(directory: &Path) -> Command {
    let mut command = git_command();
    command.arg("-C").arg(directory);
    command.args(["fetch", "--quiet", "--prune", "origin"]);
    command
}

async fn checkout(repo: &RepoUrl, directory: &Path) -> Result<(), Reason> {
    let cloned = exists(&directory.join(".git")).await?;
    let command = if cloned {
        fetch_command(directory)
    } else {
        clone_command(repo, directory)
    };
    let output = cli::run(command, String::new(), GIT_TIMEOUT).await?;
    cli::succeeded(&output, "git")
}

async fn current(path: &Path) -> Result<Option<String>, Reason> {
    match tokio::fs::read_to_string(path).await {
        Ok(text) => Ok(Some(text)),
        Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(failure) => Err(Reason::new(&failure.kind())),
    }
}

async fn replace(path: &Path, text: &FileText) -> Result<(), Reason> {
    let kind = |failure: std::io::Error| Reason::new(&failure.kind());
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(kind)?;
    }
    let temporary = path.with_extension("partial");
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)
        .await
        .map_err(kind)?;
    file.write_all(text.as_str().as_bytes())
        .await
        .map_err(kind)?;
    file.flush().await.map_err(kind)?;
    tokio::fs::rename(&temporary, path).await.map_err(kind)
}

async fn store(path: &Path, text: &FileText, existing: Existing) -> Result<Written, Reason> {
    let old = current(path).await?;
    match (old, existing) {
        (Some(_), Existing::Keep) => Ok(Written::Unchanged),
        (Some(old), _) if old == text.as_str() => Ok(Written::Unchanged),
        _ => replace(path, text).await.map(|()| Written::Changed),
    }
}

impl HostFiles for LocalHost {
    async fn ensure_age_key(&self) -> Result<HostKey, BootstrapError> {
        let path = &self.layout.key;
        generate(path).await.map_err(BootstrapError::AgeKey)?;
        let recipient = recipient_of(path).await.map_err(BootstrapError::AgeKey)?;
        let location = HostPath::new(path.to_string_lossy())?;
        Ok(HostKey::new(recipient, location))
    }

    async fn ensure_checkout(&self, repo: &RepoUrl) -> Result<(), BootstrapError> {
        let done = checkout(repo, &self.layout.checkout).await;
        done.map_err(BootstrapError::Checkout)
    }

    async fn write(
        &self,
        file: &RenderedFile,
        existing: Existing,
    ) -> Result<Written, BootstrapError> {
        let path = self.layout.stacks.join(file.path().as_str());
        let stored = store(&path, file.text(), existing).await;
        stored.map_err(BootstrapError::Write)
    }
}
