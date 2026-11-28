//! The `aegis` command line: argument parsing and the commands behind it.

pub mod args;
pub mod bootstrap;

use std::io::Write;
use std::process::ExitCode;

use args::Invocation;

/// Where a message goes.
enum Stream {
    Out,
    Err,
}

const USAGE: &str = "usage: aegis bootstrap --repo <git url> --host <name> --controller <url> \
[--with-controller --enroll-tags <tag,...> [--agents <host=url,...>]] [--checkout <dir>] [--stacks <dir>] [--image-tag <tag>]

Reads AEGIS_TS_CLIENT_ID and AEGIS_TS_CLIENT_SECRET from the environment.
";

fn report(text: &str, stream: &Stream) -> ExitCode {
    let written = match stream {
        Stream::Out => write!(std::io::stdout(), "{text}"),
        Stream::Err => write!(std::io::stderr(), "{text}"),
    };
    match (written, stream) {
        (Ok(()), Stream::Out) => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    }
}

fn bootstrap(flags: &args::Flags) -> ExitCode {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build();
    let Ok(runtime) = runtime else {
        return report("aegis: cannot start the async runtime\n", &Stream::Err);
    };
    match runtime.block_on(bootstrap::run(flags)) {
        Ok(done) => report(&done.to_string(), &Stream::Out),
        Err(failure) => report(&format!("aegis: {failure}\n"), &Stream::Err),
    }
}

/// Run the command `arguments` name (the command line without the program name) and give the exit
/// code of the process.
#[must_use]
pub fn execute(arguments: &[String]) -> ExitCode {
    match args::parse(arguments) {
        Ok(Invocation::Bootstrap(flags)) => bootstrap(&flags),
        Ok(Invocation::Help) => report(USAGE, &Stream::Out),
        Err(failure) => report(&format!("aegis: {failure}\n{USAGE}"), &Stream::Err),
    }
}
