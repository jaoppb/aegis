//! The delete script run for real against a scratch directory.

use std::path::{Path, PathBuf};

use super::REMOVE_SCRIPT;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// Runs the delete script for `path` against a host root at `root`; its exit code.
async fn run_script(path: &str, root: &Path) -> Option<i32> {
    let status = tokio::process::Command::new("sh")
        .args(["-c", REMOVE_SCRIPT, "sh", path])
        .arg(root)
        .status()
        .await
        .unwrap();
    status.code()
}

async fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("aegis-script-{}-{name}", std::process::id()));
    tokio::fs::remove_dir_all(&root).await.ok();
    tokio::fs::create_dir_all(root.join("srv/data/web/inner"))
        .await
        .unwrap();
    root
}

async fn make_dirs(root: &Path, names: &[&str]) {
    for name in names {
        let directory = root.join("srv/data").join(name);
        tokio::fs::create_dir_all(directory).await.unwrap();
    }
}

async fn plain() {
    let root = scratch("plain").await;
    assert_eq!(run_script("/srv/data/web", &root).await, Some(0));
    assert!(!root.join("srv/data/web").exists());
    assert!(root.join("srv/data").exists());
    tokio::fs::remove_dir_all(&root).await.ok();
}

async fn missing() {
    let root = scratch("missing").await;
    assert_eq!(run_script("/srv/data/gone", &root).await, Some(0));
    tokio::fs::remove_dir_all(&root).await.ok();
}

async fn linked() {
    let root = scratch("link").await;
    let elsewhere = root.join("elsewhere/web");
    tokio::fs::create_dir_all(&elsewhere).await.unwrap();
    std::os::unix::fs::symlink(root.join("elsewhere"), root.join("srv/link")).unwrap();
    assert_eq!(run_script("/srv/link/web", &root).await, Some(2));
    assert!(elsewhere.exists());
    tokio::fs::remove_dir_all(&root).await.ok();
}

async fn a_file() {
    let root = scratch("file").await;
    tokio::fs::write(root.join("srv/data/file"), "x")
        .await
        .unwrap();
    assert_eq!(run_script("/srv/data/file", &root).await, Some(3));
    assert!(root.join("srv/data/file").exists());
    tokio::fs::remove_dir_all(&root).await.ok();
}

async fn globs() {
    let root = scratch("glob").await;
    make_dirs(&root, &["a*", "abc"]).await;
    assert_eq!(run_script("/srv/data/a*", &root).await, Some(0));
    assert!(root.join("srv/data/abc").exists());
    assert!(!root.join("srv/data/a*").exists());
    tokio::fs::remove_dir_all(&root).await.ok();
}

async fn spaces() {
    let root = scratch("space").await;
    make_dirs(&root, &["we b"]).await;
    assert_eq!(run_script("/srv/data/we b", &root).await, Some(0));
    assert!(!root.join("srv/data/we b").exists());
    tokio::fs::remove_dir_all(&root).await.ok();
}

async fn leaf_link() {
    let root = scratch("leaf").await;
    let target = root.join("srv/data/web");
    std::os::unix::fs::symlink(&target, root.join("srv/data/leaf")).unwrap();
    assert_eq!(run_script("/srv/data/leaf", &root).await, Some(2));
    assert!(target.join("inner").exists());
    tokio::fs::remove_dir_all(&root).await.ok();
}

#[test]
fn the_delete_script_removes_a_plain_directory() {
    runtime().block_on(plain());
}

#[test]
fn the_delete_script_treats_a_missing_path_as_deleted() {
    runtime().block_on(missing());
}

#[test]
fn the_delete_script_never_follows_a_symlink_in_any_segment() {
    runtime().block_on(linked());
}

#[test]
fn the_delete_script_leaves_a_file_alone() {
    runtime().block_on(a_file());
}

#[test]
fn the_delete_script_handles_spaces_and_globs_without_touching_siblings() {
    runtime().block_on(globs());
    runtime().block_on(spaces());
}

#[test]
fn the_delete_script_leaves_a_leaf_symlink_alone() {
    runtime().block_on(leaf_link());
}
