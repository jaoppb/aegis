//! What a check reports.

use std::fmt;
use std::path::{Path, PathBuf};

/// The rules the checks enforce, named as they are in `AGENTS.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// A struct has more than four fields.
    StructFields,
    /// A multi-field struct in `domain` or `application` has a primitive field.
    PrimitiveField,
    /// A multi-field struct in `domain` or `application` mixes a collection with other fields.
    CollectionField,
    /// A struct field is `pub` or `pub(...)`.
    PubField,
    /// A source file is over the line limit.
    FileSize,
    /// A function signature holds a tuple.
    TupleSignature,
    /// A type alias holds a tuple.
    TupleAlias,
    /// A struct field holds a tuple.
    TupleField,
    /// A struct is named with a bag suffix.
    BagName,
    /// An `allow` or `expect` attribute.
    LintException,
    /// An arch-lint suppression comment.
    SuppressionComment,
    /// A dependency is declared outside `[workspace.dependencies]`.
    ManifestDependency,
    /// A workspace dependency version is not exact.
    ManifestVersion,
}

impl Rule {
    /// The kebab-case name used in reports and in the `AGENTS.md` rule table.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::StructFields => "struct-fields",
            Self::PrimitiveField => "primitive-field",
            Self::CollectionField => "collection-field",
            Self::PubField => "pub-field",
            Self::FileSize => "file-size",
            Self::TupleSignature => "tuple-signature",
            Self::TupleAlias => "tuple-alias",
            Self::TupleField => "tuple-field",
            Self::BagName => "bag-name",
            Self::LintException => "lint-exception",
            Self::SuppressionComment => "suppression-comment",
            Self::ManifestDependency => "manifest-dependency",
            Self::ManifestVersion => "manifest-version",
        }
    }
}

/// A file and the line a violation points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    path: PathBuf,
    line: usize,
}

impl Location {
    /// The line `line` of the file at `path`, relative to the workspace root.
    #[must_use]
    pub fn new(path: &Path, line: usize) -> Self {
        Self {
            path: path.to_path_buf(),
            line,
        }
    }
}

impl fmt::Display for Location {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.path.display(), self.line)
    }
}

/// One broken rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    rule: Rule,
    location: Location,
    message: String,
}

impl Violation {
    /// A violation of `rule` at `location`.
    #[must_use]
    pub fn new(rule: Rule, location: Location, message: impl Into<String>) -> Self {
        Self {
            rule,
            location,
            message: message.into(),
        }
    }

    /// The broken rule.
    #[must_use]
    pub const fn rule(&self) -> Rule {
        self.rule
    }
}

impl fmt::Display for Violation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let rule = self.rule.name();
        write!(formatter, "{} [{rule}] {}", self.location, self.message)
    }
}

/// Every violation found, in a stable order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report(Vec<Violation>);

impl Report {
    /// A report of `violations`, sorted by location and rule.
    #[must_use]
    pub fn new(mut violations: Vec<Violation>) -> Self {
        violations.sort_by_key(ToString::to_string);
        Self(violations)
    }

    /// Whether nothing is broken.
    #[must_use]
    pub const fn is_clean(&self) -> bool {
        self.0.is_empty()
    }

    /// The rules broken, in report order, with repeats.
    #[must_use]
    pub fn rules(&self) -> Vec<Rule> {
        self.0.iter().map(Violation::rule).collect()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0
            .iter()
            .try_for_each(|violation| writeln!(formatter, "{violation}"))
    }
}
