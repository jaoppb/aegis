//! The `std::fs` repository reader, on a scratch directory.

use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use aegis_adapters::FsRepo;
use aegis_application::{EntryKind, RepoSource};
use aegis_domain::RepoPath;

/// An empty directory unique to `name`.
fn scratch(name: &str) -> std::io::Result<PathBuf> {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[test]
fn entries_are_directories_or_other() {
    let dir = scratch("fs-entries").unwrap();
    std::fs::create_dir(dir.join("sub")).unwrap();
    std::fs::write(dir.join("file.txt"), "x").unwrap();
    let mut entries = FsRepo::new(&dir)
        .list_dir(&RepoPath::new(".").unwrap())
        .unwrap();
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    let kinds: Vec<(&str, EntryKind)> = entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry.kind))
        .collect();
    assert_eq!(
        kinds,
        [
            ("file.txt", EntryKind::Other),
            ("sub", EntryKind::Directory)
        ]
    );
}

#[test]
fn a_symlinked_directory_is_not_a_directory() {
    let dir = scratch("fs-symlink-dir").unwrap();
    let outside = scratch("fs-symlink-dir-outside").unwrap();
    symlink(&outside, dir.join("escape")).unwrap();
    let entries = FsRepo::new(&dir)
        .list_dir(&RepoPath::new(".").unwrap())
        .unwrap();
    assert_eq!(entries[0].kind, EntryKind::Other);
}

#[test]
fn a_symlinked_file_is_not_read() {
    let dir = scratch("fs-symlink-file").unwrap();
    let outside = scratch("fs-symlink-file-outside").unwrap();
    std::fs::write(outside.join("secret"), "age-key").unwrap();
    symlink(outside.join("secret"), dir.join("compose.yaml")).unwrap();
    let error = FsRepo::new(&dir).read_file(&RepoPath::new("compose.yaml").unwrap());
    assert!(error.is_err());
}

#[test]
fn a_regular_file_is_read() {
    let dir = scratch("fs-read").unwrap();
    std::fs::write(dir.join("compose.yaml"), "services: {}").unwrap();
    let text = FsRepo::new(&dir).read_file(&RepoPath::new("compose.yaml").unwrap());
    assert_eq!(text.unwrap(), "services: {}");
}

#[test]
fn invalid_utf8_is_a_read_error() {
    let dir = scratch("fs-utf8").unwrap();
    std::fs::write(dir.join("compose.yaml"), [0xff, 0xfe]).unwrap();
    assert!(
        FsRepo::new(&dir)
            .read_file(&RepoPath::new("compose.yaml").unwrap())
            .is_err()
    );
}

#[test]
fn an_oversized_file_is_refused() {
    let dir = scratch("fs-large").unwrap();
    std::fs::write(dir.join("compose.yaml"), vec![b'a'; 1024 * 1024 + 1]).unwrap();
    assert!(
        FsRepo::new(&dir)
            .read_file(&RepoPath::new("compose.yaml").unwrap())
            .is_err()
    );
}

#[test]
fn listing_a_file_is_an_error() {
    let dir = scratch("fs-list-file").unwrap();
    std::fs::write(dir.join("compose.yaml"), "").unwrap();
    assert!(
        FsRepo::new(&dir)
            .list_dir(&RepoPath::new("compose.yaml").unwrap())
            .is_err()
    );
}
