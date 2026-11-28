//! Reading the checkout of the repository an operator points at.

use std::path::{Path, PathBuf};

use aegis_application::{Outcome, Repository};

use crate::{FsRepo, SerdeStackParser, SopsCheck};

/// The path is not a checkout of an Aegis repository.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CheckoutError {
    /// The path is not a directory.
    #[error("{0} is not a directory")]
    NotADirectory(PathBuf),
    /// The directory has neither `hosts/` nor `aegis/`, so it is not a repository of stacks.
    #[error("{0} has no hosts/ or aegis/ directory, so it is not an Aegis repository")]
    NotARepository(PathBuf),
}

/// Load and validate the stacks of the checkout at `path`, offline: the loader, the validation
/// rules and the compose allowlist, as a reconcile applies them.
///
/// # Errors
///
/// Returns a [`CheckoutError`] when `path` is not a directory that holds `hosts/` or `aegis/`,
/// which a checkout of the repository does.
pub fn load_checkout(path: &Path) -> Result<Outcome, CheckoutError> {
    if !path.is_dir() {
        return Err(CheckoutError::NotADirectory(path.to_path_buf()));
    }
    let holds_stacks = path.join("hosts").is_dir() || path.join("aegis").is_dir();
    if !holds_stacks {
        return Err(CheckoutError::NotARepository(path.to_path_buf()));
    }
    let repository = Repository::new(FsRepo::new(path), SerdeStackParser, SopsCheck);
    Ok(repository.load())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("aegis-checkout-{}-{name}", std::process::id()));
        std::fs::remove_dir_all(&path).ok();
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn a_path_that_is_not_a_directory_is_refused() {
        let missing = scratch("missing").join("nowhere");
        assert_eq!(
            load_checkout(&missing),
            Err(CheckoutError::NotADirectory(missing))
        );
    }

    #[test]
    fn a_directory_without_hosts_or_aegis_is_not_a_repository() {
        let empty = scratch("empty");
        assert_eq!(
            load_checkout(&empty),
            Err(CheckoutError::NotARepository(empty))
        );
    }

    #[test]
    fn a_directory_with_a_hosts_directory_loads() {
        let repo = scratch("hosts");
        let stack = repo.join("hosts/h/web");
        std::fs::create_dir_all(&stack).unwrap();
        std::fs::write(
            stack.join("compose.yaml"),
            "services:\n  web:\n    image: nginx:1.27\n",
        )
        .unwrap();
        let outcome = load_checkout(&repo).unwrap();
        assert!(outcome.rejected().is_empty());
        assert_eq!(outcome.state().hosts().len(), 1);
    }
}
