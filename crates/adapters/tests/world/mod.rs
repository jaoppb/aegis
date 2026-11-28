//! A controller hub that has loaded the desired state of the host `h` from a scratch git origin,
//! for the tests of the routes that check a request against it.

use std::path::{Path, PathBuf};

use aegis_adapters::api::{AgentAddress, Hub};
use aegis_adapters::{BaseUrl, FsRepo, GitFollower, Remote, SerdeStackParser, SopsCheck};
use aegis_application::{MintLimits, Poller, Repository};
use aegis_domain::HostName;
use tokio::process::Command;

async fn git(directory: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_DIR")
        .args(["-c", "commit.gpgsign=false", "-c", "user.name=t"])
        .args(["-c", "user.email=t@example.com", "-C"])
        .arg(directory)
        .args(arguments)
        .output()
        .await
        .unwrap();
    assert!(output.status.success(), "git {arguments:?} failed");
}

async fn write_files(origin: &Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let target = origin.join(path);
        let parent = target.parent().unwrap();
        tokio::fs::create_dir_all(parent).await.unwrap();
        tokio::fs::write(target, text).await.unwrap();
    }
}

async fn origin_with(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!("aegis-world-{}-{name}", std::process::id()));
    tokio::fs::remove_dir_all(&root).await.ok();
    let origin = root.join("origin");
    tokio::fs::create_dir_all(&origin).await.unwrap();
    git(&origin, &["init", "-q", "-b", "trunk"]).await;
    write_files(&origin, files).await;
    git(&origin, &["add", "-A"]).await;
    git(&origin, &["commit", "-q", "-m", "state"]).await;
    root
}

/// A hub for the hosts `h` and `g` (whose agents are nowhere) that has loaded `files`, keeping keys within
/// `limits`.
///
/// # Panics
///
/// When the scratch repository cannot be built: the test cannot run.
pub async fn loaded_hub(name: &str, files: &[(&str, &str)], limits: MintLimits) -> Hub {
    let root = origin_with(name, files).await;
    let work = root.join("work");
    let remote = Remote::new(root.join("origin").to_str().unwrap(), "trunk").unwrap();
    let repository = Repository::new(FsRepo::new(&work), SerdeStackParser, SopsCheck);
    let poller = Poller::new(GitFollower::new(&work, remote), repository);
    let nowhere = |name: &str| {
        let host = HostName::new(name).unwrap();
        AgentAddress::new(host, BaseUrl::new("http://127.0.0.1:1"))
    };
    let hub = Hub::limited(&[nowhere("h"), nowhere("g")], limits).unwrap();
    hub.tick(&poller).await;
    hub
}
