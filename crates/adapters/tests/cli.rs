//! The shared process runner against children that do not read their stdin.

use std::time::Duration;

use aegis_adapters::cli::run;
use tokio::process::Command;

#[cfg(test)]
fn block_on<Output>(future: impl std::future::Future<Output = Output>) -> Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(future)
}

#[cfg(test)]
fn shell(script: &str) -> Command {
    let mut command = Command::new("sh");
    command.args(["-c", script]);
    command
}

#[cfg(test)]
mod children {
    use super::*;

    const LIMIT: Duration = Duration::from_secs(10);

    #[test]
    fn a_child_that_exits_without_reading_a_large_input_is_judged_by_its_status() {
        let input = "x".repeat(4 * 1024 * 1024);
        let output = block_on(run(shell("exit 0"), input, LIMIT)).unwrap();
        assert!(output.status.success());
    }

    #[test]
    fn a_failing_child_that_ignores_its_input_reports_its_own_status() {
        let input = "x".repeat(4 * 1024 * 1024);
        let output = block_on(run(shell("exit 3"), input, LIMIT)).unwrap();
        assert_eq!(output.status.code(), Some(3));
    }

    #[test]
    fn an_empty_input_gives_the_child_no_stdin_to_wait_on() {
        let output = block_on(run(shell("cat; echo done"), String::new(), LIMIT)).unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout), "done\n");
    }

    #[test]
    fn a_child_that_reads_its_input_gets_all_of_it() {
        let output = block_on(run(shell("cat"), "hello".to_owned(), LIMIT)).unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout), "hello");
    }
}
