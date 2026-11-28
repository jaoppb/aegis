//! The command line: which command runs and the `--flag value` pairs it was given.
//!
//! Every command reads its own flags from [`Flags`], so a new command adds a variant to
//! [`Invocation`] and changes nothing else here.

use std::collections::BTreeMap;

use aegis_domain::ValueError;

/// The flags one command accepts: switches take no value, valued flags take one.
struct Spec {
    switches: &'static [&'static str],
    valued: &'static [&'static str],
}

/// The flags of `aegis bootstrap`.
const BOOTSTRAP: Spec = Spec {
    switches: &["--with-controller"],
    valued: &[
        "--repo",
        "--host",
        "--controller",
        "--checkout",
        "--stacks",
        "--image-tag",
        "--enroll-tags",
    ],
};

/// The command line is not one Aegis understands.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UsageError {
    /// The first word is not a command.
    #[error("unknown command `{0}`")]
    UnknownCommand(String),
    /// A word that is not a known flag.
    #[error("unknown argument `{0}`")]
    UnknownArgument(String),
    /// A flag that needs a value has none.
    #[error("{0} needs a value")]
    MissingValue(String),
    /// A required flag is absent.
    #[error("{0} is required")]
    Required(&'static str),
    /// A tag a stack's node may carry is one of Aegis's own role tags.
    #[error("{0} is reserved for Aegis itself and cannot be a stack tag")]
    ReservedTag(String),
    /// A flag holds a value that is not valid.
    #[error("{flag}: {cause}")]
    Invalid {
        /// The flag.
        flag: &'static str,
        /// Why the value was refused.
        cause: ValueError,
    },
}

/// The flags of one command, by name. A switch has an empty value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Flags(BTreeMap<String, String>);

impl Flags {
    fn with(mut self, name: &str, value: &str) -> Self {
        self.0.insert(name.to_owned(), value.to_owned());
        self
    }

    /// The value of the flag `name`, if it was given.
    #[must_use]
    pub fn value(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }

    /// The value of the flag `name`.
    ///
    /// # Errors
    ///
    /// Returns [`UsageError::Required`] when the flag was not given.
    pub fn required(&self, name: &'static str) -> Result<&str, UsageError> {
        self.value(name).ok_or(UsageError::Required(name))
    }

    /// Whether the switch `name` was given.
    #[must_use]
    pub fn has(&self, name: &str) -> bool {
        self.0.contains_key(name)
    }
}

/// What the command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    /// `aegis bootstrap`, with its flags.
    Bootstrap(Flags),
    /// Print the usage.
    Help,
}

fn collect(rest: &[String], spec: &Spec, flags: Flags) -> Result<Flags, UsageError> {
    match rest {
        [] => Ok(flags),
        [switch, tail @ ..] if spec.switches.contains(&switch.as_str()) => {
            collect(tail, spec, flags.with(switch, ""))
        }
        [name, value, tail @ ..] if spec.valued.contains(&name.as_str()) => {
            collect(tail, spec, flags.with(name, value))
        }
        [name] if spec.valued.contains(&name.as_str()) => {
            Err(UsageError::MissingValue(name.clone()))
        }
        [other, ..] => Err(UsageError::UnknownArgument(other.clone())),
    }
}

/// Read the command line, without the program name.
///
/// # Errors
///
/// Returns a [`UsageError`] for an unknown command or flag, or a flag without its value.
pub fn parse(arguments: &[String]) -> Result<Invocation, UsageError> {
    match arguments {
        [] => Ok(Invocation::Help),
        [command, rest @ ..] if command == "bootstrap" => {
            collect(rest, &BOOTSTRAP, Flags::default()).map(Invocation::Bootstrap)
        }
        [command, ..] if matches!(command.as_str(), "help" | "--help" | "-h") => {
            Ok(Invocation::Help)
        }
        [command, ..] => Err(UsageError::UnknownCommand(command.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn no_command_prints_the_usage() {
        assert_eq!(parse(&[]).unwrap(), Invocation::Help);
    }

    #[test]
    fn bootstrap_reads_valued_flags_and_switches() {
        let parsed = parse(&words("bootstrap --repo git@host:r.git --with-controller")).unwrap();
        let flags = match parsed {
            Invocation::Bootstrap(flags) => flags,
            Invocation::Help => panic!("not a bootstrap"),
        };
        assert_eq!(flags.value("--repo"), Some("git@host:r.git"));
        assert!(flags.has("--with-controller"));
        assert_eq!(flags.value("--host"), None);
    }

    #[test]
    fn an_unknown_command_is_refused() {
        assert_eq!(
            parse(&words("sync")),
            Err(UsageError::UnknownCommand("sync".to_owned()))
        );
    }

    #[test]
    fn an_unknown_flag_is_refused() {
        assert_eq!(
            parse(&words("bootstrap --force")),
            Err(UsageError::UnknownArgument("--force".to_owned()))
        );
    }

    #[test]
    fn a_valued_flag_without_a_value_is_refused() {
        assert_eq!(
            parse(&words("bootstrap --host")),
            Err(UsageError::MissingValue("--host".to_owned()))
        );
    }

    #[test]
    fn a_required_flag_that_is_absent_is_named() {
        let flags = Flags::default();
        assert_eq!(
            flags.required("--repo"),
            Err(UsageError::Required("--repo"))
        );
    }
}
