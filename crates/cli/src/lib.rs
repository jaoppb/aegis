//! The `aegis` command line: argument parsing and the commands behind it.

pub mod args;
pub mod bootstrap;
pub mod exit;
pub mod operate;

use std::io::Write;
use std::process::ExitCode;

use args::Invocation;
use exit::Output;

const USAGE: &str = "usage: aegis <command>

commands:
  validate [path]                                  check a checkout of the repository, offline
  status [--controller <url>]                      the commit, hosts and stacks the controller knows
  diff [path] [--controller <url>]                 the checkout against what the hosts run
  sync [--host <name>] [--stack <name>] [--controller <url>]
                                                   converge now, instead of at the next poll
  bootstrap --repo <git url> --host <name> --controller <url>
            [--with-controller --enroll-tags <tag,...> [--agents <host=url,...>]]
            [--checkout <dir>] [--stacks <dir>] [--image-tag <tag>]
                                                   install Aegis on this host

status, diff and sync need the controller URL from --controller or AEGIS_CONTROLLER, and call it
with your Tailscale identity: your login must be listed in AEGIS_OPERATORS on the controller.
bootstrap reads AEGIS_TS_CLIENT_ID and AEGIS_TS_CLIENT_SECRET from the environment.

exit codes: 0 clean, 1 a check found something, 2 the command could not complete
";

fn bootstrap(flags: &args::Flags) -> Output {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build();
    let Ok(runtime) = runtime else {
        return Output::failed("aegis: cannot start the async runtime\n");
    };
    match runtime.block_on(bootstrap::run(flags)) {
        Ok(done) => Output::said(done.to_string()),
        Err(failure) => Output::failed(format!("aegis: {failure}\n")),
    }
}

/// The output of an operator command: what it rendered, or why it could not complete.
fn operated(result: Result<aegis_adapters::operator::Rendered, operate::Failure>) -> Output {
    match result {
        Ok(rendered) => Output::ran(&rendered),
        Err(failure) => Output::failed(format!("aegis: {failure}\n")),
    }
}

/// Run the command `arguments` name (the command line without the program name) and say what it
/// printed and how it ended, without printing anything itself.
#[must_use]
pub fn run(arguments: &[String]) -> Output {
    match args::parse(arguments) {
        Ok(Invocation::Bootstrap(flags)) => bootstrap(&flags),
        Ok(Invocation::Validate(flags)) => operated(operate::validate(&flags)),
        Ok(Invocation::Status(flags)) => operated(operate::status(&flags)),
        Ok(Invocation::Diff(flags)) => operated(operate::diff(&flags)),
        Ok(Invocation::Sync(flags)) => operated(operate::sync(&flags)),
        Ok(Invocation::Help) => Output::said(USAGE),
        Err(failure) => Output::failed(format!("aegis: {failure}\n{USAGE}")),
    }
}

/// Print what a command produced, and give the exit code of the process. A command that cannot
/// write its output could not complete.
fn emit(output: &Output) -> ExitCode {
    let out = write!(std::io::stdout(), "{}", output.out());
    let err = write!(std::io::stderr(), "{}", output.err());
    if out.is_err() || err.is_err() {
        return ExitCode::from(exit::Exit::Failed.code());
    }
    output.code()
}

/// Run the command `arguments` name (the command line without the program name) and give the exit
/// code of the process: `0` clean, `1` a check found something, `2` the command could not
/// complete.
#[must_use]
pub fn execute(arguments: &[String]) -> ExitCode {
    emit(&run(arguments))
}
