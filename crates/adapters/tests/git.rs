//! The git adapter against a real local repository and clone.

use std::path::{Path, PathBuf};

use aegis_adapters::GitCheckout;
use aegis_application::Checkout;
use aegis_domain::CommitSha;
use tokio::process::Command;

#[cfg(test)]
async fn git(directory: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_DIR")
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-C",
        ])
        .arg(directory)
        .args(arguments)
        .output()
        .await
        .unwrap();
    assert!(output.status.success(), "git {arguments:?} failed");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// A fresh `origin` repository with one commit, and a clone of it, under a scratch directory.
#[cfg(test)]
struct Pair {
    root: PathBuf,
    origin: PathBuf,
    checkout: PathBuf,
}

#[cfg(test)]
impl Pair {
    async fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("aegis-git-{}-{name}", std::process::id()));
        tokio::fs::remove_dir_all(&root).await.ok();
        let origin = root.join("origin");
        tokio::fs::create_dir_all(&origin).await.unwrap();
        git(&origin, &["init", "-q"]).await;
        let pair = Self {
            checkout: root.join("checkout"),
            origin,
            root,
        };
        pair.commit("one").await;
        let source = pair.origin.to_str().unwrap();
        let target = pair.checkout.to_str().unwrap();
        git(&pair.root, &["clone", "-q", source, target]).await;
        pair
    }

    fn at(root: PathBuf) -> Self {
        Self {
            origin: root.join("origin"),
            checkout: root.join("checkout"),
            root,
        }
    }

    async fn commit(&self, text: &str) -> String {
        tokio::fs::write(self.origin.join("file"), text)
            .await
            .unwrap();
        git(&self.origin, &["add", "file"]).await;
        git(&self.origin, &["commit", "-q", "-m", text]).await;
        git(&self.origin, &["rev-parse", "HEAD"]).await
    }

    async fn head(&self) -> String {
        git(&self.checkout, &["rev-parse", "HEAD"]).await
    }

    async fn sync(&self, sha: &str) -> Result<(), String> {
        let target = CommitSha::new(sha).map_err(|failure| failure.to_string())?;
        let done = GitCheckout::new(&self.checkout).sync(&target).await;
        done.map_err(|error| error.to_string())
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
fn a_checkout_fetches_and_moves_to_a_new_commit_detached() {
    runtime().block_on(async {
        let pair = Pair::new("moves").await;
        let first = pair.head().await;
        let second = pair.commit("two").await;
        pair.sync(&second).await.unwrap();
        assert_eq!(pair.head().await, second);
        let text = tokio::fs::read_to_string(pair.checkout.join("file")).await;
        assert_eq!(text.unwrap(), "two");
        pair.sync(&first).await.unwrap();
        assert_eq!(pair.head().await, first);
    });
}

#[test]
fn an_unknown_commit_is_an_error_that_does_not_echo_git_output() {
    runtime().block_on(async {
        let pair = Pair::new("unknown").await;
        let failure = pair.sync(&"0".repeat(40)).await.unwrap_err();
        assert!(failure.contains("git exited with"));
        assert!(!failure.contains(pair.origin.to_str().unwrap()));
    });
}

/// Runs only inside the child process of the next test, where the parent set `GIT_DIR`.
#[test]
fn redirected_git_child() {
    let Ok(root) = std::env::var("AEGIS_CHILD_PAIR") else {
        return;
    };
    runtime().block_on(async {
        let pair = Pair::at(root.into());
        let head = pair.head().await;
        pair.sync(&head).await.unwrap();
    });
}

#[test]
fn a_redirecting_git_variable_of_the_agent_does_not_reach_git() {
    runtime().block_on(async {
        let pair = Pair::new("redirect").await;
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "redirected_git_child"])
            .env("AEGIS_CHILD_PAIR", &pair.root)
            .env("GIT_DIR", "/nonexistent/aegis-git-dir")
            .status()
            .unwrap();
        assert!(status.success());
    });
}
