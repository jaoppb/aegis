//! The aegis binary.

use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    aegis::execute(&arguments)
}
