//! How a command ends and what it printed: the exit codes of the `aegis` command line.
//!
//! `0` means clean, `1` means a check found something, `2` means the command could not complete
//! (a bad flag, an unreadable path, a controller that cannot be reached or refuses the caller).

use std::process::ExitCode;

use aegis_adapters::operator::{Finding, Rendered};

/// How a command ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// The command ran and found nothing.
    Clean,
    /// The command ran and a check found something.
    Found,
    /// The command could not complete.
    Failed,
}

impl Exit {
    /// The exit code of the process: `0`, `1` or `2`.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Clean => 0,
            Self::Found => 1,
            Self::Failed => 2,
        }
    }
}

impl From<Finding> for Exit {
    fn from(finding: Finding) -> Self {
        match finding {
            Finding::Clean => Self::Clean,
            Finding::Found => Self::Found,
        }
    }
}

/// What a command printed and how it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    out: String,
    err: String,
    exit: Exit,
}

impl Output {
    /// A command that ran: `rendered` is its text, and its finding the exit code.
    #[must_use]
    pub fn ran(rendered: &Rendered) -> Self {
        Self {
            out: rendered.text().to_owned(),
            err: String::new(),
            exit: rendered.finding().into(),
        }
    }

    /// A command that printed `text` and found nothing.
    #[must_use]
    pub fn said(text: impl Into<String>) -> Self {
        Self {
            out: text.into(),
            err: String::new(),
            exit: Exit::Clean,
        }
    }

    /// A command that could not complete, for `reason`.
    #[must_use]
    pub fn failed(reason: impl Into<String>) -> Self {
        Self {
            out: String::new(),
            err: reason.into(),
            exit: Exit::Failed,
        }
    }

    /// What goes to standard output.
    #[must_use]
    pub fn out(&self) -> &str {
        &self.out
    }

    /// What goes to standard error.
    #[must_use]
    pub fn err(&self) -> &str {
        &self.err
    }

    /// How the command ended.
    #[must_use]
    pub const fn exit(&self) -> Exit {
        self.exit
    }

    /// The exit code of the process.
    #[must_use]
    pub fn code(&self) -> ExitCode {
        ExitCode::from(self.exit.code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_exit_codes_are_zero_one_and_two() {
        assert_eq!(Exit::Clean.code(), 0);
        assert_eq!(Exit::Found.code(), 1);
        assert_eq!(Exit::Failed.code(), 2);
    }

    #[test]
    fn a_finding_maps_to_its_exit() {
        assert_eq!(Exit::from(Finding::Clean), Exit::Clean);
        assert_eq!(Exit::from(Finding::Found), Exit::Found);
    }

    #[test]
    fn a_failure_prints_to_standard_error_only() {
        let output = Output::failed("nope");
        assert_eq!(output.err(), "nope");
        assert_eq!(output.out(), "");
        assert_eq!(output.exit(), Exit::Failed);
    }
}
