//! The aegis-agent binary.

use std::process::ExitCode;

fn main() -> ExitCode {
    // Reading the arguments keeps `main` non-const until the real entry point replaces this stub.
    std::env::args_os().for_each(drop);
    ExitCode::SUCCESS
}
