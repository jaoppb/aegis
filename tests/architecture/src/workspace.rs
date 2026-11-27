//! The files of a workspace, read and parsed once for every check.

use std::path::{Path, PathBuf};

use syn::visit::Visit;
use syn::{ItemMod, ItemStruct};

use crate::error::CheckError;

/// Directory names that never hold workspace code to check.
const SKIPPED_DIRS: [&str; 3] = ["target", "fixtures", "node_modules"];
/// The top-level directories that hold Rust code and manifests.
const ROOTS: [&str; 2] = ["crates", "tests"];

/// Which architecture layer a source file belongs to, decided by its path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// `crates/domain`.
    Domain,
    /// `crates/application`.
    Application,
    /// `crates/adapters`.
    Adapters,
    /// Any other crate.
    Other,
}

impl Layer {
    fn of(parts: &[&str]) -> Self {
        match parts {
            ["crates", "domain", ..] => Self::Domain,
            ["crates", "application", ..] => Self::Application,
            ["crates", "adapters", ..] => Self::Adapters,
            _ => Self::Other,
        }
    }

    /// Whether the primitive and collection field rules apply.
    #[must_use]
    pub const fn is_modelled(self) -> bool {
        matches!(self, Self::Domain | Self::Application)
    }
}

/// What a Rust file is for: crate `src` code, which the shape rules apply to, or support code such
/// as integration tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// A `src` file of a crate.
    Production,
    /// An integration test or any other file.
    Support,
}

impl Role {
    fn of(parts: &[&str]) -> Self {
        match parts {
            ["crates", _, "src", ..] | ["tests", "architecture", "src", ..] => Self::Production,
            _ => Self::Support,
        }
    }
}

/// Where a source file sits: its path, its layer and its role.
#[derive(Debug)]
struct Origin {
    path: PathBuf,
    layer: Layer,
    role: Role,
}

impl Origin {
    fn of(path: &Path) -> Self {
        let parts: Vec<&str> = path.iter().filter_map(|part| part.to_str()).collect();
        Self {
            path: path.to_path_buf(),
            layer: Layer::of(&parts),
            role: Role::of(&parts),
        }
    }
}

/// A Rust source file with its text and its syntax tree.
#[derive(Debug)]
pub struct SourceFile {
    origin: Origin,
    text: String,
    syntax: syn::File,
}

impl SourceFile {
    /// Parse `text`, the contents of the file at `path` relative to the workspace root.
    ///
    /// # Errors
    ///
    /// Returns [`CheckError::Syntax`] when `text` is not valid Rust.
    pub fn parse(path: &Path, text: String) -> Result<Self, CheckError> {
        let syntax = syn::parse_file(&text).map_err(|source| CheckError::Syntax {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(Self {
            origin: Origin::of(path),
            text,
            syntax,
        })
    }

    /// The path relative to the workspace root.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.origin.path
    }

    /// The file contents.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The layer the file belongs to.
    #[must_use]
    pub const fn layer(&self) -> Layer {
        self.origin.layer
    }

    /// Whether the file is crate `src` code, which the shape rules apply to.
    #[must_use]
    pub fn is_production(&self) -> bool {
        self.origin.role == Role::Production
    }

    /// The syntax tree.
    #[must_use]
    pub const fn syntax(&self) -> &syn::File {
        &self.syntax
    }

    /// Every struct outside `#[cfg(test)]` modules, nested ones included.
    #[must_use]
    pub fn structs(&self) -> Vec<&ItemStruct> {
        let mut collector = StructCollector::default();
        collector.visit_file(&self.syntax);
        collector.structs
    }
}

/// Whether `item` is a module compiled only for tests.
#[must_use]
pub fn is_test_module(item: &ItemMod) -> bool {
    item.attrs.iter().any(|attribute| {
        attribute.path().is_ident("cfg")
            && attribute
                .parse_args::<syn::Ident>()
                .is_ok_and(|ident| ident == "test")
    })
}

#[derive(Default)]
struct StructCollector<'ast> {
    structs: Vec<&'ast ItemStruct>,
}

impl<'ast> Visit<'ast> for StructCollector<'ast> {
    fn visit_item_mod(&mut self, item: &'ast ItemMod) {
        (!is_test_module(item)).then(|| syn::visit::visit_item_mod(self, item));
    }

    fn visit_item_struct(&mut self, item: &'ast ItemStruct) {
        self.structs.push(item);
        syn::visit::visit_item_struct(self, item);
    }
}

/// A parsed `Cargo.toml`.
#[derive(Debug)]
pub struct Manifest {
    path: PathBuf,
    table: toml::Table,
}

impl Manifest {
    /// Parse `text`, the contents of the manifest at `path` relative to the workspace root.
    ///
    /// # Errors
    ///
    /// Returns [`CheckError::Manifest`] when `text` is not valid TOML.
    pub fn parse(path: &Path, text: &str) -> Result<Self, CheckError> {
        let table = text.parse().map_err(|source| CheckError::Manifest {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(Self {
            path: path.to_path_buf(),
            table,
        })
    }

    /// The path relative to the workspace root.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The parsed document.
    #[must_use]
    pub const fn table(&self) -> &toml::Table {
        &self.table
    }

    /// Whether this is the workspace root manifest.
    #[must_use]
    pub fn is_root(&self) -> bool {
        self.path == Path::new("Cargo.toml")
    }
}

/// Every Rust file and manifest under a workspace root.
#[derive(Debug, Default)]
pub struct Workspace {
    sources: Vec<SourceFile>,
    manifests: Vec<Manifest>,
}

impl Workspace {
    /// A workspace made of already parsed files.
    #[must_use]
    pub const fn new(sources: Vec<SourceFile>, manifests: Vec<Manifest>) -> Self {
        Self { sources, manifests }
    }

    /// Read every `.rs` file under `crates/` and `tests/`, and every `Cargo.toml` there and at the
    /// root. Build output and fixture directories are skipped.
    ///
    /// # Errors
    ///
    /// Returns [`CheckError`] when a file cannot be read or parsed.
    pub fn open(root: &Path) -> Result<Self, CheckError> {
        let mut files = vec![PathBuf::from("Cargo.toml")];
        ROOTS
            .iter()
            .try_for_each(|top| walk(root, Path::new(top), &mut files))?;
        let mut workspace = Self::default();
        files
            .iter()
            .try_for_each(|relative| workspace.add(root, relative))?;
        Ok(workspace)
    }

    fn add(&mut self, root: &Path, relative: &Path) -> Result<(), CheckError> {
        let text =
            std::fs::read_to_string(root.join(relative)).map_err(|source| CheckError::Read {
                path: relative.to_path_buf(),
                source,
            })?;
        match relative.extension().and_then(std::ffi::OsStr::to_str) {
            Some("rs") => self.sources.push(SourceFile::parse(relative, text)?),
            _ => self.manifests.push(Manifest::parse(relative, &text)?),
        }
        Ok(())
    }

    /// The Rust sources.
    #[must_use]
    pub fn sources(&self) -> &[SourceFile] {
        &self.sources
    }

    /// The manifests.
    #[must_use]
    pub fn manifests(&self) -> &[Manifest] {
        &self.manifests
    }
}

fn is_wanted(path: &Path) -> bool {
    let name = path.file_name().and_then(|name| name.to_str());
    name == Some("Cargo.toml") || path.extension().is_some_and(|ext| ext == "rs")
}

fn is_skipped(name: &str) -> bool {
    name.starts_with('.') || SKIPPED_DIRS.contains(&name)
}

/// Collect the wanted files under `relative`, as paths relative to `root`.
fn walk(root: &Path, relative: &Path, files: &mut Vec<PathBuf>) -> Result<(), CheckError> {
    let read = |source| CheckError::Read {
        path: relative.to_path_buf(),
        source,
    };
    let entries = std::fs::read_dir(root.join(relative)).map_err(read)?;
    for entry in entries {
        let entry = entry.map_err(read)?;
        visit_entry(root, &relative.join(entry.file_name()), files)?;
    }
    Ok(())
}

fn visit_entry(root: &Path, path: &Path, files: &mut Vec<PathBuf>) -> Result<(), CheckError> {
    let name = path.file_name().and_then(|name| name.to_str());
    if name.is_none_or(is_skipped) {
        return Ok(());
    }
    if root.join(path).is_dir() {
        return walk(root, path, files);
    }
    if is_wanted(path) {
        files.push(path.to_path_buf());
    }
    Ok(())
}
