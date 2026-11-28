//! The command line: which command runs and the `--flag value` pairs it was given.
//!
//! Every command reads its own flags from [`Flags`], so a new command adds a variant to
//! [`Invocation`] and changes nothing else here.

use std::collections::BTreeMap;

use aegis_domain::ValueError;

/// The flags one command accepts: switches take no value, valued flags take one, and a command
/// may take one path as its first word.
struct Spec {
    switches: &'static [&'static str],
    valued: &'static [&'static str],
    first: First,
}

/// What a command may take as its first word.
#[derive(Clone, Copy, PartialEq, Eq)]
enum First {
    /// Only flags.
    Flag,
    /// A path, then flags.
    Path,
}

/// The name a command's path argument goes by in [`Flags`].
const PATH: &str = "<path>";

/// The flag that says where the controller is.
const CONTROLLER_FLAG: &str = "--controller";

/// The flags of `aegis validate`: a path and nothing else.
const VALIDATE: Spec = Spec {
    switches: &[],
    valued: &[],
    first: First::Path,
};

/// The flags of `aegis status`.
const STATUS: Spec = Spec {
    switches: &[],
    valued: &[CONTROLLER_FLAG],
    first: First::Flag,
};

/// The flags of `aegis diff`: a path and the controller.
const DIFF: Spec = Spec {
    switches: &[],
    valued: &[CONTROLLER_FLAG],
    first: First::Path,
};

/// The flags of `aegis sync`.
const SYNC: Spec = Spec {
    switches: &[],
    valued: &[CONTROLLER_FLAG, "--host", "--stack"],
    first: First::Flag,
};

/// The flags of `aegis bootstrap`.
const BOOTSTRAP: Spec = Spec {
    switches: &["--with-controller"],
    first: First::Flag,
    valued: &[
        "--repo",
        "--host",
        "--controller",
        "--checkout",
        "--stacks",
        "--image-tag",
        "--enroll-tags",
        "--agents",
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

    /// The path the command was given as its first word, if any.
    #[must_use]
    pub fn path(&self) -> Option<&str> {
        self.value(PATH)
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
    /// `aegis validate [path]`.
    Validate(Flags),
    /// `aegis status`.
    Status(Flags),
    /// `aegis diff [path]`.
    Diff(Flags),
    /// `aegis sync`.
    Sync(Flags),
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
        [word, tail @ ..]
            if spec.first == First::Path && !word.starts_with("--") && !flags.has(PATH) =>
        {
            collect(tail, spec, flags.with(PATH, word))
        }
        [other, ..] => Err(UsageError::UnknownArgument(other.clone())),
    }
}

/// A command by its name: the flags it accepts and what it makes of them.
struct Command {
    name: &'static str,
    spec: Spec,
    invoke: fn(Flags) -> Invocation,
}

const COMMANDS: [Command; 5] = [
    Command {
        name: "bootstrap",
        spec: BOOTSTRAP,
        invoke: Invocation::Bootstrap,
    },
    Command {
        name: "validate",
        spec: VALIDATE,
        invoke: Invocation::Validate,
    },
    Command {
        name: "status",
        spec: STATUS,
        invoke: Invocation::Status,
    },
    Command {
        name: "diff",
        spec: DIFF,
        invoke: Invocation::Diff,
    },
    Command {
        name: "sync",
        spec: SYNC,
        invoke: Invocation::Sync,
    },
];

/// Read the command line, without the program name.
///
/// # Errors
///
/// Returns a [`UsageError`] for an unknown command or flag, or a flag without its value.
pub fn parse(arguments: &[String]) -> Result<Invocation, UsageError> {
    let [command, rest @ ..] = arguments else {
        return Ok(Invocation::Help);
    };
    let known = COMMANDS.iter().find(|each| each.name == command);
    match known {
        Some(found) => collect(rest, &found.spec, Flags::default()).map(found.invoke),
        None if matches!(command.as_str(), "help" | "--help" | "-h") => Ok(Invocation::Help),
        None => Err(UsageError::UnknownCommand(command.clone())),
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
            other => panic!("not a bootstrap: {other:?}"),
        };
        assert_eq!(flags.value("--repo"), Some("git@host:r.git"));
        assert!(flags.has("--with-controller"));
        assert_eq!(flags.value("--host"), None);
    }

    #[test]
    fn an_unknown_command_is_refused() {
        assert_eq!(
            parse(&words("frobnicate")),
            Err(UsageError::UnknownCommand("frobnicate".to_owned()))
        );
    }

    fn flags_of(line: &str) -> Flags {
        match parse(&words(line)) {
            Ok(
                Invocation::Validate(flags)
                | Invocation::Diff(flags)
                | Invocation::Sync(flags)
                | Invocation::Status(flags),
            ) => flags,
            other => panic!("not an operator command: {other:?}"),
        }
    }

    #[test]
    fn validate_and_diff_take_an_optional_path() {
        assert_eq!(flags_of("validate ./repo").path(), Some("./repo"));
        let diff = flags_of("diff ./repo --controller http://c:7480");
        assert_eq!(diff.path(), Some("./repo"));
        assert_eq!(diff.value("--controller"), Some("http://c:7480"));
        assert_eq!(flags_of("validate").path(), None);
    }

    #[test]
    fn a_second_path_is_refused() {
        assert_eq!(
            parse(&words("validate a b")),
            Err(UsageError::UnknownArgument("b".to_owned()))
        );
    }

    #[test]
    fn status_and_sync_take_no_path_and_sync_takes_a_host_and_a_stack() {
        assert_eq!(
            parse(&words("status ./repo")),
            Err(UsageError::UnknownArgument("./repo".to_owned()))
        );
        let flags = flags_of("sync --host h --stack web");
        assert_eq!(flags.value("--host"), Some("h"));
        assert_eq!(flags.value("--stack"), Some("web"));
    }

    #[test]
    fn a_flag_of_another_command_is_refused_by_validate() {
        assert_eq!(
            parse(&words("validate --controller http://c")),
            Err(UsageError::UnknownArgument("--controller".to_owned()))
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
