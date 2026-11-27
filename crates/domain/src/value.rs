//! Validated value types: newtypes over the primitives the desired state is made of.

use std::fmt;
use std::num::{NonZeroU16, NonZeroUsize};

/// Why a raw value was rejected by a newtype constructor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueError {
    /// The value is empty.
    Empty,
    /// The value contains a character a name may not contain.
    ForbiddenChar(char),
    /// A path that must be relative is absolute.
    NotRelative,
    /// A path that must be absolute is relative.
    NotAbsolute,
    /// A path climbs out of its root with a `..` segment.
    Traversal,
    /// A path holds a `$`, so Compose would interpolate it after validation.
    Interpolation,
    /// A tailnet tag does not start with `tag:` or has no name after it.
    BadTag,
    /// A port is zero.
    ZeroPort,
}

impl fmt::Display for ValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("value is empty"),
            Self::ForbiddenChar(found) => write!(formatter, "forbidden character {found:?}"),
            Self::NotRelative => formatter.write_str("path must be relative"),
            Self::NotAbsolute => formatter.write_str("path must be absolute"),
            Self::Traversal => formatter.write_str("path must not contain `..`"),
            Self::Interpolation => formatter.write_str("path must not contain `$`"),
            Self::BadTag => formatter.write_str("tailnet tag must look like `tag:<name>`"),
            Self::ZeroPort => formatter.write_str("port must not be zero"),
        }
    }
}

impl std::error::Error for ValueError {}

const fn is_name_char(candidate: char) -> bool {
    candidate.is_ascii_alphanumeric() || matches!(candidate, '-' | '_' | '.')
}

/// A name usable as a directory, compose service or volume name. A leading `.` is rejected so a
/// dotfile never passes for a host or a stack.
fn validate_name(value: &str) -> Result<(), ValueError> {
    if value.is_empty() {
        return Err(ValueError::Empty);
    }
    let forbidden = value.chars().enumerate().find(|&(position, candidate)| {
        !is_name_char(candidate) || (position == 0 && candidate == '.')
    });
    forbidden.map_or(Ok(()), |(_, candidate)| {
        Err(ValueError::ForbiddenChar(candidate))
    })
}

/// Reject `..` segments, and `$` because Compose interpolates it after validation.
fn reject_unsafe_path(value: &str) -> Result<(), ValueError> {
    if value.contains('$') {
        return Err(ValueError::Interpolation);
    }
    if value.split('/').any(|segment| segment == "..") {
        return Err(ValueError::Traversal);
    }
    Ok(())
}

fn validate_relative(value: &str) -> Result<(), ValueError> {
    if value.is_empty() {
        return Err(ValueError::Empty);
    }
    if value.starts_with('/') {
        return Err(ValueError::NotRelative);
    }
    reject_unsafe_path(value)
}

fn validate_declared(value: &str) -> Result<(), ValueError> {
    validate_non_empty(value)?;
    if value.contains('$') {
        return Err(ValueError::Interpolation);
    }
    Ok(())
}

fn validate_absolute(value: &str) -> Result<(), ValueError> {
    if !value.starts_with('/') {
        return Err(ValueError::NotAbsolute);
    }
    reject_unsafe_path(value)
}

const fn validate_non_empty(value: &str) -> Result<(), ValueError> {
    if value.is_empty() {
        return Err(ValueError::Empty);
    }
    Ok(())
}

fn validate_tag(value: &str) -> Result<(), ValueError> {
    let name = value.strip_prefix("tag:").ok_or(ValueError::BadTag)?;
    validate_name(name).map_err(|_| ValueError::BadTag)
}

macro_rules! string_newtype {
    ($(#[$meta:meta])* $name:ident, $validate:path) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            /// Validate `value` and wrap it.
            ///
            /// # Errors
            ///
            /// Returns a [`ValueError`] when `value` is not valid for this type.
            pub fn new(value: impl Into<String>) -> Result<Self, ValueError> {
                let value = value.into();
                $validate(&value)?;
                Ok(Self(value))
            }

            /// The validated string.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

string_newtype!(
    /// The name of a host: the directory under `hosts/`.
    HostName,
    validate_name
);
string_newtype!(
    /// The name of a stack: the directory under a host.
    StackName,
    validate_name
);
string_newtype!(
    /// The name of a compose service.
    ServiceName,
    validate_name
);
string_newtype!(
    /// The name of a compose named volume.
    VolumeName,
    validate_name
);
string_newtype!(
    /// A path relative to the repository root, or to a stack directory, that never leaves it.
    RepoPath,
    validate_relative
);
string_newtype!(
    /// A path exactly as a compose file declares it: it may be absolute or climb out of the stack
    /// directory, which is what validation checks.
    DeclaredPath,
    validate_declared
);
string_newtype!(
    /// An absolute path on the host.
    HostPath,
    validate_absolute
);
string_newtype!(
    /// An absolute path inside a container.
    ContainerPath,
    validate_absolute
);
string_newtype!(
    /// A Tailscale ACL tag such as `tag:aegis-agent`.
    TailnetTag,
    validate_tag
);

string_newtype!(
    /// The image reference of a service, such as `ghcr.io/jaoppb/aegis-agent:1.2`.
    ImageRef,
    validate_non_empty
);
/// The text of a `compose.yaml` exactly as written in the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeSource(String);

impl ComposeSource {
    /// Wrap `text` unchanged.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// The text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A sequence of `$item` values that is a type of its own, so no struct mixes a collection with
/// other fields.
macro_rules! collection {
    ($(#[$meta:meta])* $name:ident, $item:ty) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Default, PartialEq, Eq)]
        pub struct $name(Vec<$item>);

        impl $name {
            /// Wrap `items`, keeping their order.
            #[must_use]
            pub const fn new(items: Vec<$item>) -> Self {
                Self(items)
            }

            /// The items in order.
            pub fn iter(&self) -> std::slice::Iter<'_, $item> {
                self.0.iter()
            }

            /// The items as a slice.
            #[must_use]
            pub fn as_slice(&self) -> &[$item] {
                &self.0
            }

            /// The number of items.
            #[must_use]
            pub const fn len(&self) -> usize {
                self.0.len()
            }

            /// Whether there are no items.
            #[must_use]
            pub const fn is_empty(&self) -> bool {
                self.0.is_empty()
            }
        }

        impl IntoIterator for $name {
            type Item = $item;
            type IntoIter = std::vec::IntoIter<$item>;

            fn into_iter(self) -> Self::IntoIter {
                self.0.into_iter()
            }
        }

        impl<'items> IntoIterator for &'items $name {
            type Item = &'items $item;
            type IntoIter = std::slice::Iter<'items, $item>;

            fn into_iter(self) -> Self::IntoIter {
                self.0.iter()
            }
        }

        impl FromIterator<$item> for $name {
            fn from_iter<Source: IntoIterator<Item = $item>>(source: Source) -> Self {
                Self(source.into_iter().collect())
            }
        }
    };
}

pub(crate) use collection;

impl RepoPath {
    /// Append one directory or file name to this path.
    ///
    /// # Errors
    ///
    /// Returns a [`ValueError`] when `segment` is not a valid name.
    pub fn join(&self, segment: &str) -> Result<Self, ValueError> {
        validate_name(segment)?;
        Self::new(format!("{}/{segment}", self.0))
    }
}

impl RepoPath {
    /// The `hosts` directory that holds the user stacks.
    #[must_use]
    pub fn hosts_root() -> Self {
        Self("hosts".to_owned())
    }

    /// The reserved `aegis` directory that holds Aegis's own stacks.
    #[must_use]
    pub fn reserved_root() -> Self {
        Self("aegis".to_owned())
    }

    /// The path of an entry `name` read from a directory listing or fixed by the layout. Unlike
    /// [`RepoPath::join`] it does not judge the name, so a badly named directory can still be
    /// reported.
    #[must_use]
    pub fn listed(&self, name: &str) -> Self {
        Self(format!("{}/{name}", self.0))
    }

    /// The path of `relative` inside this directory, with `.` segments dropped.
    ///
    /// # Errors
    ///
    /// Returns a [`ValueError`] when the joined path is not a valid [`RepoPath`].
    pub fn within(&self, relative: &Self) -> Result<Self, ValueError> {
        let kept = relative
            .as_str()
            .split('/')
            .filter(|segment| !matches!(*segment, "" | "."));
        let joined = kept.fold(self.0.clone(), |path, segment| format!("{path}/{segment}"));
        Self::new(joined)
    }
}

impl DeclaredPath {
    /// The path as a [`RepoPath`], or `None` when it is absolute or climbs out with `..`.
    #[must_use]
    pub fn inside_stack(&self) -> Option<RepoPath> {
        RepoPath::new(self.as_str()).ok()
    }
}

/// Whether `needle` occurs in `line` with no path or name character on either side.
fn holds_token(line: &str, needle: &str) -> bool {
    let joins = |character: Option<char>| {
        character.is_some_and(|found| is_name_char(found) || found == '/')
    };
    line.match_indices(needle).any(|(start, found)| {
        let before = line.get(..start).and_then(|head| head.chars().next_back());
        let after = line.get(start.saturating_add(found.len())..);
        !joins(before) && !joins(after.and_then(|tail| tail.chars().next()))
    })
}

/// A 1-based line number in a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Line(NonZeroUsize);

impl Line {
    /// The line `number`, counting from 1, or `None` for zero.
    #[must_use]
    pub fn new(number: usize) -> Option<Self> {
        NonZeroUsize::new(number).map(Self)
    }

    /// The best line of `text` for `needle`: the first list item (`- ...`) that holds it as a
    /// whole token, else the first line that does, else `None`. It is a guess, not a parse.
    #[must_use]
    pub fn of(text: &str, needle: &str) -> Option<Self> {
        let holds = |line: &&str| holds_token(line, needle);
        let item = text
            .lines()
            .position(|line| line.trim_start().starts_with("- ") && holds(&line));
        let index = item.or_else(|| text.lines().position(|line| holds(&line)))?;
        Self::new(index.saturating_add(1))
    }
}

impl fmt::Display for Line {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// A non-zero TCP port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Port(NonZeroU16);

impl Port {
    /// Wrap `value`.
    ///
    /// # Errors
    ///
    /// Returns [`ValueError::ZeroPort`] when `value` is zero.
    pub fn new(value: u16) -> Result<Self, ValueError> {
        NonZeroU16::new(value).map(Self).ok_or(ValueError::ZeroPort)
    }

    /// The port number.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0.get()
    }
}

impl fmt::Display for Port {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_accept_compose_style_identifiers() {
        assert!(HostName::new("jao-server").is_ok());
        assert!(ServiceName::new("clip_cascade.web").is_ok());
    }

    #[test]
    fn names_reject_empty_dotfiles_and_separators() {
        assert_eq!(StackName::new(""), Err(ValueError::Empty));
        assert_eq!(StackName::new(".git"), Err(ValueError::ForbiddenChar('.')));
        assert_eq!(StackName::new("a/b"), Err(ValueError::ForbiddenChar('/')));
        assert_eq!(StackName::new("a b"), Err(ValueError::ForbiddenChar(' ')));
    }

    #[test]
    fn repo_paths_are_relative_and_stay_inside_the_root() {
        assert!(RepoPath::new("./config/app.toml").is_ok());
        assert_eq!(RepoPath::new("/etc"), Err(ValueError::NotRelative));
        assert_eq!(RepoPath::new("a/../b"), Err(ValueError::Traversal));
        assert_eq!(RepoPath::new(""), Err(ValueError::Empty));
    }

    #[test]
    fn paths_reject_interpolation() {
        assert_eq!(
            HostPath::new("/srv/${DIR}/x"),
            Err(ValueError::Interpolation)
        );
        assert_eq!(RepoPath::new("./$DIR"), Err(ValueError::Interpolation));
        assert_eq!(
            ContainerPath::new("/data/$X"),
            Err(ValueError::Interpolation)
        );
    }

    #[test]
    fn host_paths_must_be_absolute() {
        assert!(HostPath::new("/srv/data").is_ok());
        assert_eq!(HostPath::new("data"), Err(ValueError::NotAbsolute));
        assert_eq!(HostPath::new("/srv/../etc"), Err(ValueError::Traversal));
    }

    #[test]
    fn join_appends_a_valid_segment_only() {
        let root = RepoPath::new("hosts").unwrap();
        assert_eq!(root.join("jao").unwrap().as_str(), "hosts/jao");
        assert_eq!(root.join("a/b"), Err(ValueError::ForbiddenChar('/')));
    }

    #[test]
    fn tags_need_the_tag_prefix_and_a_name() {
        assert!(TailnetTag::new("tag:aegis-agent").is_ok());
        assert_eq!(TailnetTag::new("aegis"), Err(ValueError::BadTag));
        assert_eq!(TailnetTag::new("tag:"), Err(ValueError::BadTag));
    }

    #[test]
    fn collections_keep_order_and_report_their_size() {
        let names = ["a", "b"].map(|name| VolumeName::new(name).unwrap());
        let volumes: crate::Volumes = names.into_iter().collect();
        assert_eq!(volumes.len(), 2);
        assert!(!volumes.is_empty());
        assert!(crate::Volumes::default().is_empty());
        let listed: Vec<&str> = volumes.iter().map(VolumeName::as_str).collect();
        assert_eq!(listed, ["a", "b"]);
        assert_eq!(volumes.as_slice().len(), 2);
    }

    #[test]
    fn within_joins_and_drops_dot_segments() {
        let stack = RepoPath::new("hosts/h/s").unwrap();
        let rel = |text| RepoPath::new(text).unwrap();
        assert_eq!(
            stack.within(&rel("./a/./b")).unwrap().as_str(),
            "hosts/h/s/a/b"
        );
        assert_eq!(stack.within(&rel(".")).unwrap().as_str(), "hosts/h/s");
    }

    #[test]
    fn declared_paths_may_escape_but_report_it() {
        assert!(DeclaredPath::new("./app").unwrap().inside_stack().is_some());
        assert!(
            DeclaredPath::new("../app")
                .unwrap()
                .inside_stack()
                .is_none()
        );
        assert!(DeclaredPath::new("/abs").unwrap().inside_stack().is_none());
        assert_eq!(DeclaredPath::new(""), Err(ValueError::Empty));
        assert_eq!(DeclaredPath::new("$X"), Err(ValueError::Interpolation));
    }

    #[test]
    fn lines_count_from_one_and_find_the_first_match() {
        assert_eq!(Line::of("a\nb\nb", "b"), Line::new(2));
        assert_eq!(Line::of("x: ./data.env\n- .env\n", ".env"), Line::new(2));
        let shared = "postgres:\n  image: postgres:16\n  volumes:\n    - postgres:/data\n";
        assert_eq!(Line::of(shared, "postgres"), Line::new(4));
        assert_eq!(Line::of("a", "z"), None);
        assert_eq!(Line::new(0), None);
    }

    #[test]
    fn ports_reject_zero() {
        assert_eq!(Port::new(0), Err(ValueError::ZeroPort));
        assert_eq!(Port::new(8080).unwrap().get(), 8080);
    }
}
