//! Runs an external program with a timeout and without ever keeping its argv, environment or
//! output, so nothing a command carries can leak into an error.

use std::process::{Output, Stdio};
use std::time::Duration;

use aegis_application::Reason;
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, Command};

/// Run `command` to completion within `limit`, feeding `input` on its stdin. The child is killed
/// when the call is dropped or times out.
///
/// # Errors
///
/// Returns a [`Reason`] that names only the failure kind, never the command line or its output.
pub async fn run(mut command: Command, input: String, limit: Duration) -> Result<Output, Reason> {
    command.kill_on_drop(true);
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = command.spawn().map_err(|failure| kind(&failure))?;
    let finished = tokio::time::timeout(limit, feed_and_wait(child, input)).await;
    finished.map_err(|_| Reason::new(&"timed out"))?
}

async fn feed_and_wait(mut child: Child, input: String) -> Result<Output, Reason> {
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(input.as_bytes())
            .await
            .map_err(|failure| kind(&failure))?;
    }
    child
        .wait_with_output()
        .await
        .map_err(|failure| kind(&failure))
}

fn kind(failure: &std::io::Error) -> Reason {
    Reason::new(&failure.kind())
}
