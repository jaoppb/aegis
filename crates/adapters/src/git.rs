//! The `git` CLI adapter of the [`Checkout`] port. The controller's git polling reuses it.

use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use aegis_application::{Checkout, GitError, Reason, Upstream};
use aegis_domain::CommitSha;
use tokio::process::Command;

use crate::cli;

/// The environment git inherits from the agent, and nothing else: no `GIT_DIR`, `GIT_WORK_TREE`
/// or `GIT_SSH_COMMAND` can redirect it.
const INHERITED: [&str; 6] = [
    "PATH",
    "HOME",
    "SSH_AUTH_SOCK",
    "XDG_CONFIG_HOME",
    "SSL_CERT_FILE",
    "GIT_SSL_CAINFO",
];

/// The transports git may use, so a URL cannot start another `git-remote-*` helper.
const ALLOWED_PROTOCOLS: &str = "https:ssh:file";

/// How long one git command may take.
const GIT_TIMEOUT: Duration = Duration::from_secs(120);

/// Moves a local git working tree to a commit by running `git`.
///
/// Neither the command lines, the environment nor git's output are kept in errors or logs: a
/// remote URL may carry credentials.
#[derive(Debug, Clone)]
pub struct GitCheckout {
    program: String,
    directory: PathBuf,
}

/// The remote branch a controller follows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    url: String,
    branch: String,
}

/// A remote that cannot be followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RemoteError {
    /// The URL is empty, starts with `-` or holds whitespace or control characters.
    #[error("the repository URL is empty or malformed")]
    BadUrl,
    /// The URL carries credentials; use a deploy key or a git credential helper instead.
    #[error("the repository URL carries credentials")]
    Credentials,
    /// The branch name is empty or not a plain branch name.
    #[error("the branch name is empty or malformed")]
    BadBranch,
}

impl Remote {
    /// The `branch` of the repository at `url`.
    ///
    /// # Errors
    ///
    /// Returns a [`RemoteError`] when either is malformed, or when the URL holds a password or a
    /// token (user info of an `http` or `https` URL).
    pub fn new(url: &str, branch: &str) -> Result<Self, RemoteError> {
        check_url(url)?;
        check_branch(branch)?;
        Ok(Self {
            url: url.to_owned(),
            branch: branch.to_owned(),
        })
    }

    fn refspec(&self) -> String {
        format!("+refs/heads/{}", self.branch)
    }
}

fn plain(text: &str) -> bool {
    !text.is_empty()
        && !text.starts_with('-')
        && !text
            .chars()
            .any(|found| found.is_whitespace() || found.is_control())
}

fn check_url(url: &str) -> Result<(), RemoteError> {
    if !plain(url) {
        return Err(RemoteError::BadUrl);
    }
    if url.contains("::") {
        return Err(RemoteError::BadUrl);
    }
    let Some((scheme, rest)) = url.split_once("://") else {
        return Ok(());
    };
    let authority = rest.split('/').next().unwrap_or_default();
    let Some((userinfo, _host)) = authority.rsplit_once('@') else {
        return Ok(());
    };
    let web = matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https");
    if web || userinfo.contains(':') {
        return Err(RemoteError::Credentials);
    }
    Ok(())
}

fn check_branch(branch: &str) -> Result<(), RemoteError> {
    let forbidden = ['~', '^', ':', '?', '*', '[', '\\'];
    let bad = !plain(branch)
        || branch.contains("..")
        || branch.contains("@{")
        || branch.ends_with('/')
        || branch.to_ascii_lowercase().ends_with(".lock")
        || branch.chars().any(|found| forbidden.contains(&found));
    if bad {
        return Err(RemoteError::BadBranch);
    }
    Ok(())
}

impl GitCheckout {
    /// The working tree in `directory`, driven by `git` from the `PATH`.
    #[must_use]
    pub fn new<Directory: Into<PathBuf>>(directory: Directory) -> Self {
        Self::with_program("git", directory)
    }

    /// The working tree in `directory`, driven by the `program` executable.
    #[must_use]
    pub fn with_program<Program: Into<String>, Directory: Into<PathBuf>>(
        program: Program,
        directory: Directory,
    ) -> Self {
        Self {
            program: program.into(),
            directory: directory.into(),
        }
    }

    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(&self.program);
        command.arg("-C").arg(&self.directory).args(arguments);
        cli::inherit_only(&mut command, &INHERITED);
        command.env("GIT_TERMINAL_PROMPT", "0");
        command.env("GIT_ALLOW_PROTOCOL", ALLOWED_PROTOCOLS);
        command
    }

    async fn step(&self, arguments: &[&str]) -> Result<(), GitError> {
        let output = cli::run(self.command(arguments), String::new(), GIT_TIMEOUT).await;
        output
            .and_then(|finished| succeeded(&finished))
            .map_err(GitError::new)
    }
}

/// The output of a command that succeeded.
fn stdout_of(output: &Output) -> Result<String, Reason> {
    succeeded(output)?;
    String::from_utf8(output.stdout.clone())
        .map(|text| text.trim().to_owned())
        .map_err(|_| Reason::new(&"git printed invalid UTF-8"))
}

fn succeeded(output: &Output) -> Result<(), Reason> {
    if output.status.success() {
        return Ok(());
    }
    Err(Reason::new(&format!("git exited with {}", output.status)))
}

impl GitCheckout {
    async fn capture(&self, arguments: &[&str]) -> Result<String, GitError> {
        let output = cli::run(self.command(arguments), String::new(), GIT_TIMEOUT).await;
        output
            .and_then(|finished| stdout_of(&finished))
            .map_err(GitError::new)
    }

    async fn detach(&self, commit: &CommitSha) -> Result<(), GitError> {
        let detach = [
            "checkout",
            "--quiet",
            "--force",
            "--detach",
            commit.as_str(),
            "--",
        ];
        self.step(&detach).await
    }

    /// Make sure the directory is the top of a git repository, starting over when it is missing
    /// or damaged. Only a `.git` directory is ever removed, never other files.
    async fn prepare(&self) -> Result<(), GitError> {
        let top = self.capture(&["rev-parse", "--git-dir"]).await;
        match top.as_deref() {
            Ok(".git") => Ok(()),
            _ => self.start_over().await,
        }
    }

    async fn start_over(&self) -> Result<(), GitError> {
        let failed = |error: std::io::Error| GitError::new(Reason::new(&error.kind()));
        let adoptable = is_adoptable(&self.directory).await.map_err(failed)?;
        adoptable
            .then_some(())
            .ok_or_else(|| GitError::new(Reason::new(&"the work directory is not empty")))?;
        let removed = tokio::fs::remove_dir_all(self.directory.join(".git")).await;
        removed.or_else(forgive_missing).map_err(failed)?;
        let created = tokio::fs::create_dir_all(&self.directory).await;
        created.map_err(failed)?;
        self.step(&["init", "--quiet"]).await
    }
}

/// Whether `directory` may become the work directory: it is missing, empty or already a git
/// repository. Anything else could be somebody's data, which `git clean` would delete.
async fn is_adoptable(directory: &Path) -> std::io::Result<bool> {
    if tokio::fs::try_exists(directory.join(".git")).await? {
        return Ok(true);
    }
    let opened = tokio::fs::read_dir(directory).await;
    let mut entries = match opened {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error),
    };
    Ok(entries.next_entry().await?.is_none())
}

fn forgive_missing(error: std::io::Error) -> std::io::Result<()> {
    if error.kind() == std::io::ErrorKind::NotFound {
        return Ok(());
    }
    Err(error)
}

impl Checkout for GitCheckout {
    async fn sync(&self, commit: &CommitSha) -> Result<(), GitError> {
        self.step(&["fetch", "--quiet", "--prune", "origin"])
            .await?;
        self.detach(commit).await
    }
}

/// A working tree that follows the newest commit of a remote branch.
#[derive(Debug, Clone)]
pub struct GitFollower {
    checkout: GitCheckout,
    remote: Remote,
}

impl GitFollower {
    /// The working tree in `directory`, following `remote`.
    #[must_use]
    pub fn new<Directory: Into<PathBuf>>(directory: Directory, remote: Remote) -> Self {
        Self {
            checkout: GitCheckout::new(directory),
            remote,
        }
    }
}

impl Upstream for GitFollower {
    async fn advance(&self) -> Result<CommitSha, GitError> {
        let git = &self.checkout;
        git.prepare().await?;
        let fetch = [
            "fetch",
            "--quiet",
            self.remote.url.as_str(),
            &self.remote.refspec(),
        ];
        git.step(&fetch).await?;
        let head = git
            .capture(&["rev-parse", "--verify", "FETCH_HEAD"])
            .await?;
        let commit = CommitSha::new(head).map_err(|error| GitError::new(Reason::new(&error)))?;
        git.detach(&commit).await?;
        git.step(&["clean", "--quiet", "-ffdx"]).await?;
        Ok(commit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn a_missing_program_is_a_git_error_without_the_command_line() {
        let git = GitCheckout::with_program("/nonexistent/aegis-git", "/secret/path");
        let commit = CommitSha::new("abcdef0").unwrap();
        let error = runtime().block_on(git.sync(&commit)).unwrap_err();
        let text = error.to_string();
        assert!(!text.contains("secret"));
        assert!(!text.contains("abcdef0"));
    }
}
