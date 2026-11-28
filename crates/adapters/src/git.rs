//! The `git` CLI adapter of the [`Checkout`] port. The controller's git polling reuses it.

use std::path::PathBuf;
use std::process::Output;
use std::time::Duration;

use aegis_application::{Checkout, GitError, Reason};
use aegis_domain::CommitSha;
use tokio::process::Command;

use crate::cli;

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
        command.env("GIT_TERMINAL_PROMPT", "0");
        command
    }

    async fn step(&self, arguments: &[&str]) -> Result<(), GitError> {
        let output = cli::run(self.command(arguments), String::new(), GIT_TIMEOUT).await;
        output
            .and_then(|finished| succeeded(&finished))
            .map_err(GitError::new)
    }
}

fn succeeded(output: &Output) -> Result<(), Reason> {
    if output.status.success() {
        return Ok(());
    }
    Err(Reason::new(&format!("git exited with {}", output.status)))
}

impl Checkout for GitCheckout {
    async fn sync(&self, commit: &CommitSha) -> Result<(), GitError> {
        self.step(&["fetch", "--quiet", "--prune", "origin"])
            .await?;
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
