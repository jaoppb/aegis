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

use aegis_adapters::{GitFollower, Remote};
use aegis_application::Upstream;

#[cfg(test)]
async fn branch_of(origin: &Path) -> String {
    git(origin, &["rev-parse", "--abbrev-ref", "HEAD"]).await
}

/// A checkout in a new directory that follows `pair`'s origin.
#[cfg(test)]
async fn follower(pair: &Pair) -> GitFollower {
    let branch = branch_of(&pair.origin).await;
    let remote = Remote::new(pair.origin.to_str().unwrap(), &branch).unwrap();
    GitFollower::new(pair.root.join("follower"), remote)
}

#[test]
fn a_followed_remote_is_cloned_into_a_missing_directory_and_tracks_new_commits() {
    runtime().block_on(async {
        let pair = Pair::new("follow").await;
        let follower = follower(&pair).await;
        let first = follower.advance().await.unwrap();
        assert_eq!(
            first.as_str(),
            git(&pair.origin, &["rev-parse", "HEAD"]).await
        );
        let second = pair.commit("two").await;
        assert_eq!(follower.advance().await.unwrap().as_str(), second);
        let text = tokio::fs::read_to_string(pair.root.join("follower/file")).await;
        assert_eq!(text.unwrap(), "two");
    });
}

#[test]
fn files_the_commit_does_not_hold_are_removed_from_the_work_directory() {
    runtime().block_on(async {
        let pair = Pair::new("clean").await;
        let follower = follower(&pair).await;
        follower.advance().await.unwrap();
        let stray = pair.root.join("follower/stray");
        let nested = pair.root.join("follower/nested/deeper");
        tokio::fs::write(&stray, "leftover").await.unwrap();
        tokio::fs::create_dir_all(&nested).await.unwrap();
        tokio::fs::write(nested.join("file"), "x").await.unwrap();
        tokio::fs::write(pair.root.join("follower/file"), "edited")
            .await
            .unwrap();
        follower.advance().await.unwrap();
        assert!(!stray.exists());
        assert!(!pair.root.join("follower/nested").exists());
        let text = tokio::fs::read_to_string(pair.root.join("follower/file")).await;
        assert_eq!(text.unwrap(), "one");
    });
}

#[test]
fn a_damaged_git_directory_is_rebuilt() {
    runtime().block_on(async {
        let pair = Pair::new("damaged").await;
        let follower = follower(&pair).await;
        follower.advance().await.unwrap();
        tokio::fs::write(pair.root.join("follower/.git/HEAD"), "garbage")
            .await
            .unwrap();
        assert!(follower.advance().await.is_ok());
    });
}

#[test]
fn an_unreachable_remote_is_an_error_that_leaves_the_checkout_where_it_was() {
    runtime().block_on(async {
        let pair = Pair::new("gone").await;
        let follower = follower(&pair).await;
        let before = follower.advance().await.unwrap();
        tokio::fs::remove_dir_all(&pair.origin).await.unwrap();
        let failure = follower.advance().await.unwrap_err().to_string();
        assert!(!failure.contains(pair.origin.to_str().unwrap()));
        let head = git(&pair.root.join("follower"), &["rev-parse", "HEAD"]).await;
        assert_eq!(head, before.as_str());
    });
}

#[test]
fn a_work_directory_with_other_files_is_never_adopted() {
    runtime().block_on(async {
        let pair = Pair::new("adopt").await;
        let follower = follower(&pair).await;
        let mine = pair.root.join("follower");
        tokio::fs::create_dir_all(&mine).await.unwrap();
        tokio::fs::write(mine.join("precious"), "data")
            .await
            .unwrap();
        assert!(follower.advance().await.is_err());
        assert!(mine.join("precious").exists());
    });
}

#[test]
fn remotes_with_credentials_are_refused() {
    assert!(Remote::new("https://github.com/acme/aegis.git", "main").is_ok());
    assert!(Remote::new("git@github.com:acme/aegis.git", "main").is_ok());
    assert!(Remote::new("ssh://git@github.com/acme/aegis.git", "release/1").is_ok());
    assert!(Remote::new("/srv/git/aegis.git", "main").is_ok());
    assert!(Remote::new("https://token@github.com/acme/aegis.git", "main").is_err());
    assert!(Remote::new("https://user:pass@github.com/a.git", "main").is_err());
    assert!(Remote::new("ssh://git:secret@host/a.git", "main").is_err());
}

#[test]
fn urls_that_hide_credentials_or_transports_are_refused() {
    assert!(Remote::new("HTTPS://token@github.com/a.git", "main").is_err());
    assert!(Remote::new("https::https://token@github.com/a.git", "main").is_err());
    assert!(Remote::new("ext::sh -c evil", "main").is_err());
}

#[test]
fn remotes_with_odd_names_are_refused() {
    assert!(Remote::new("--upload-pack=evil", "main").is_err());
    assert!(Remote::new("", "main").is_err());
    assert!(Remote::new("https://host/a.git", "").is_err());
    assert!(Remote::new("https://host/a.git", "-x").is_err());
    assert!(Remote::new("https://host/a.git", "a..b").is_err());
    assert!(Remote::new("https://host/a.git", "a b").is_err());
}
