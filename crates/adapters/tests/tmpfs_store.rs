//! The tmpfs store: private modes, removal, the sweep, and the refusal of anything that is not
//! tmpfs or not a private directory of the agent.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use aegis_adapters::TmpfsStore;
use aegis_application::{SecretStore, Slot};
use aegis_domain::{ProjectName, Secret, SlotName, StackName};

#[cfg(test)]
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[cfg(test)]
fn project() -> ProjectName {
    ProjectName::of(&StackName::new("web").unwrap()).unwrap()
}

#[cfg(test)]
fn slot() -> Slot {
    Slot::new(project(), SlotName::new("env-app-0").unwrap())
}

#[cfg(test)]
fn root(name: &str) -> PathBuf {
    PathBuf::from("/dev/shm").join(format!("aegis-store-{}-{name}", std::process::id()))
}

/// A mount table that lists `root` as `kind`, written beside it.
#[cfg(test)]
async fn mount_table(root: &std::path::Path, kind: &str) -> PathBuf {
    tokio::fs::create_dir_all(root).await.unwrap();
    let canonical = tokio::fs::canonicalize(root).await.unwrap();
    let table = root.with_extension("mounts");
    let line = format!("1 0 0:1 / {} rw - {kind} none rw\n", canonical.display());
    tokio::fs::write(&table, line).await.unwrap();
    tokio::fs::remove_dir(root).await.unwrap();
    table
}

#[test]
fn files_are_private_and_removed_with_their_project() {
    runtime().block_on(async {
        let root = root("modes");
        let store = TmpfsStore::at(&root);
        let path = store.place(&slot(), &Secret::new("A=1")).await.unwrap();
        let file = tokio::fs::metadata(path.as_str()).await.unwrap();
        let directory = tokio::fs::metadata(root.join("aegis-web")).await.unwrap();
        assert_eq!(file.permissions().mode() & 0o777, 0o600);
        assert_eq!(directory.permissions().mode() & 0o777, 0o700);
        assert_eq!(
            tokio::fs::read_to_string(path.as_str()).await.unwrap(),
            "A=1"
        );
        store.discard(&project()).await.unwrap();
        assert!(!root.join("aegis-web").exists());
        store.discard(&project()).await.unwrap();
        tokio::fs::remove_dir_all(&root).await.ok();
    });
}

#[test]
fn the_sweep_removes_what_an_earlier_run_left() {
    runtime().block_on(async {
        let root = root("sweep");
        let store = TmpfsStore::at(&root);
        store.place(&slot(), &Secret::new("A=1")).await.unwrap();
        store.sweep().await.unwrap();
        assert!(!root.exists());
        store.sweep().await.unwrap();
    });
}

#[test]
fn a_directory_that_is_not_listed_as_tmpfs_is_refused_and_nothing_is_written() {
    runtime().block_on(async {
        let root = root("disk");
        let table = mount_table(&root, "ext4").await;
        let store = TmpfsStore::at(&root).with_mount_table(&table);
        let error = store.place(&slot(), &Secret::new("A=1")).await.unwrap_err();
        assert!(error.to_string().contains("not on tmpfs"), "{error}");
        assert!(!root.join("aegis-web").exists());
        tokio::fs::remove_dir_all(&root).await.ok();
        tokio::fs::remove_file(&table).await.ok();
    });
}

#[test]
fn a_directory_listed_as_tmpfs_is_accepted() {
    runtime().block_on(async {
        let root = root("listed");
        let table = mount_table(&root, "tmpfs").await;
        let store = TmpfsStore::at(&root).with_mount_table(&table);
        assert!(store.place(&slot(), &Secret::new("A=1")).await.is_ok());
        tokio::fs::remove_dir_all(&root).await.ok();
        tokio::fs::remove_file(&table).await.ok();
    });
}

#[test]
fn a_project_directory_planted_as_a_link_is_refused() {
    runtime().block_on(async {
        let root = root("planted");
        let elsewhere = root.with_extension("elsewhere");
        tokio::fs::create_dir_all(&elsewhere).await.unwrap();
        let store = TmpfsStore::at(&root);
        store.place(&slot(), &Secret::new("A=1")).await.unwrap();
        store.discard(&project()).await.unwrap();
        tokio::fs::symlink(&elsewhere, root.join("aegis-web"))
            .await
            .unwrap();
        let error = store.place(&slot(), &Secret::new("A=1")).await.unwrap_err();
        assert!(error.to_string().contains("private directory"), "{error}");
        assert!(
            tokio::fs::read_dir(&elsewhere)
                .await
                .unwrap()
                .next_entry()
                .await
                .unwrap()
                .is_none()
        );
        tokio::fs::remove_dir_all(&root).await.ok();
        tokio::fs::remove_dir_all(&elsewhere).await.ok();
    });
}

#[test]
fn a_root_that_others_can_enter_is_refused() {
    runtime().block_on(async {
        let root = root("open");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let open = std::fs::Permissions::from_mode(0o755);
        tokio::fs::set_permissions(&root, open).await.unwrap();
        let error = TmpfsStore::at(&root)
            .place(&slot(), &Secret::new("A=1"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("private directory"), "{error}");
        tokio::fs::remove_dir_all(&root).await.ok();
    });
}

#[test]
fn a_project_directory_with_loose_permissions_is_refused() {
    runtime().block_on(async {
        let root = root("loose");
        let store = TmpfsStore::at(&root);
        store.place(&slot(), &Secret::new("A=1")).await.unwrap();
        let open = std::fs::Permissions::from_mode(0o750);
        tokio::fs::set_permissions(root.join("aegis-web"), open)
            .await
            .unwrap();
        let error = store.place(&slot(), &Secret::new("A=2")).await.unwrap_err();
        assert!(error.to_string().contains("private directory"), "{error}");
        tokio::fs::remove_dir_all(&root).await.ok();
    });
}

#[test]
fn a_link_planted_at_the_file_is_replaced_and_its_target_left_alone() {
    runtime().block_on(async {
        let root = root("filelink");
        let target = root.with_extension("target");
        tokio::fs::write(&target, "untouched").await.unwrap();
        let store = TmpfsStore::at(&root);
        store.place(&slot(), &Secret::new("A=1")).await.unwrap();
        let file = root.join("aegis-web/env-app-0");
        tokio::fs::remove_file(&file).await.unwrap();
        tokio::fs::symlink(&target, &file).await.unwrap();
        store.place(&slot(), &Secret::new("A=2")).await.unwrap();
        assert_eq!(
            tokio::fs::read_to_string(&target).await.unwrap(),
            "untouched"
        );
        assert_eq!(tokio::fs::read_to_string(&file).await.unwrap(), "A=2");
        let mode = tokio::fs::metadata(&file)
            .await
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        tokio::fs::remove_dir_all(&root).await.ok();
        tokio::fs::remove_file(&target).await.ok();
    });
}

#[test]
fn placing_the_same_slot_again_replaces_the_content() {
    runtime().block_on(async {
        let root = root("again");
        let store = TmpfsStore::at(&root);
        store.place(&slot(), &Secret::new("first")).await.unwrap();
        let path = store.place(&slot(), &Secret::new("second")).await.unwrap();
        assert_eq!(
            tokio::fs::read_to_string(path.as_str()).await.unwrap(),
            "second"
        );
        tokio::fs::remove_dir_all(&root).await.ok();
    });
}

#[cfg(test)]
async fn place_and_read_back(store: &TmpfsStore, rounds: u32) {
    for round in 0..rounds {
        let text = format!("A={round}");
        let path = store
            .place(&slot(), &Secret::new(text.clone()))
            .await
            .unwrap();
        let read = tokio::fs::read_to_string(path.as_str()).await.unwrap();
        assert_eq!(read, text, "round {round}");
    }
}

#[test]
fn a_placed_secret_is_readable_at_once() {
    runtime().block_on(async {
        let root = root("flush");
        place_and_read_back(&TmpfsStore::at(&root), 200).await;
        tokio::fs::remove_dir_all(&root).await.ok();
    });
}
