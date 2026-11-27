//! Validated value types: newtypes over the primitives the desired state is made of.

use std::fmt;
use std::num::NonZeroU16;

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
    fn ports_reject_zero() {
        assert_eq!(Port::new(0), Err(ValueError::ZeroPort));
        assert_eq!(Port::new(8080).unwrap().get(), 8080);
    }
}
