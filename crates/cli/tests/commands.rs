//! The commands end to end: the arguments in, the text and the exit code out, against a fake
//! controller on a real socket.

use std::path::PathBuf;

use aegis::exit::Exit;
use aegis::run;
use aegis_api::version_1::{
    Agent, Confirmation, ControllerStatus, FLEET_PATH, HostStatus, HostSync, SYNCS_PATH,
    StackState, StackStatus, StatusReport, SyncResponse, TargetStatus,
};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};

#[cfg(test)]
fn words(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_owned).collect()
}

/// Run the server of `routes` on a runtime of this thread, once it has told where it listens.
#[cfg(test)]
async fn listen(routes: Router, sender: std::sync::mpsc::Sender<std::net::SocketAddr>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    sender.send(listener.local_addr().unwrap()).unwrap();
    axum::serve(listener, routes).await.unwrap();
}

#[cfg(test)]
fn serve_forever(routes: Router, sender: std::sync::mpsc::Sender<std::net::SocketAddr>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(listen(routes, sender));
}

/// Serve `routes` on a free local port from a thread of its own, and say where.
#[cfg(test)]
fn serve(routes: Router) -> String {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || serve_forever(routes, sender));
    format!("http://{}", receiver.recv().unwrap())
}

#[cfg(test)]
fn stack(state: StackState) -> StackStatus {
    StackStatus::new("hosts/h/web", state, Vec::new())
}

#[cfg(test)]
fn status_of(state: StackState) -> ControllerStatus {
    let report = StatusReport::new("h", "abc1234", vec![stack(state)]);
    let host = HostStatus::new("h", Agent::Mapped, Confirmation::Confirmed).with_report(report);
    ControllerStatus::new(Some(TargetStatus::new("abc1234", Vec::new())), vec![host])
}

#[cfg(test)]
fn controller_with(state: StackState) -> String {
    let status = status_of(state);
    let sync = SyncResponse::new(vec![HostSync::done(
        "h",
        StatusReport::new("h", "abc1234", vec![stack(state)]),
    )]);
    let routes = Router::new()
        .route(FLEET_PATH, get(move || async move { Json(status) }))
        .route(SYNCS_PATH, post(move || async move { Json(sync) }));
    serve(routes)
}

#[cfg(test)]
fn repo(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = std::env::temp_dir().join(format!("aegis-cli-{}-{name}", std::process::id()));
    std::fs::remove_dir_all(&root).ok();
    for (path, text) in files {
        let target = root.join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, text).unwrap();
    }
    root
}

#[cfg(test)]
const COMPOSE: &str = "services:\n  web:\n    image: nginx:1.27\n";

#[test]
fn validate_exits_zero_on_a_clean_checkout() {
    let root = repo("clean", &[("hosts/h/web/compose.yaml", COMPOSE)]);
    let output = run(&words(&format!("validate {}", root.display())));
    assert_eq!(output.exit(), Exit::Clean);
    assert_eq!(output.out(), "ok: 1 stacks\n");
}

#[test]
fn validate_exits_one_when_a_stack_is_rejected() {
    let root = repo("broken", &[("hosts/h/broken/readme", "no compose")]);
    let output = run(&words(&format!("validate {}", root.display())));
    assert_eq!(output.exit(), Exit::Found);
    assert!(output.out().contains("invalid hosts/h/broken"));
}

#[test]
fn validate_exits_two_when_the_path_cannot_be_read() {
    let output = run(&words("validate /nonexistent/aegis-checkout"));
    assert_eq!(output.exit(), Exit::Failed);
    assert!(output.err().contains("is not a directory"));
}

#[test]
fn a_bad_command_line_exits_two_and_prints_the_usage() {
    let output = run(&words("frobnicate"));
    assert_eq!(output.exit(), Exit::Failed);
    assert!(output.err().contains("unknown command `frobnicate`"));
    assert!(output.err().contains("exit codes: 0 clean"));
}

#[test]
fn the_exit_codes_are_in_the_usage() {
    let output = run(&words("help"));
    assert_eq!(output.exit(), Exit::Clean);
    assert!(
        output
            .out()
            .contains("0 clean, 1 a check found something, 2")
    );
}

#[test]
fn status_exits_zero_when_every_host_is_confirmed_and_running() {
    let url = controller_with(StackState::Running);
    let output = run(&words(&format!("status --controller {url}")));
    assert_eq!(output.exit(), Exit::Clean);
    assert!(output.out().contains("target abc1234"));
}

#[test]
fn status_exits_one_when_a_stack_failed() {
    let url = controller_with(StackState::Failed);
    let output = run(&words(&format!("status --controller {url}")));
    assert_eq!(output.exit(), Exit::Found);
}

#[test]
fn status_exits_two_when_the_controller_refuses_the_caller_and_says_why() {
    let routes = Router::new().route(FLEET_PATH, get(|| async { StatusCode::FORBIDDEN }));
    let url = serve(routes);
    let output = run(&words(&format!("status --controller {url}")));
    assert_eq!(output.exit(), Exit::Failed);
    assert!(output.err().contains("AEGIS_OPERATORS"));
}

#[test]
fn status_exits_two_when_the_controller_cannot_be_reached() {
    let output = run(&words("status --controller http://127.0.0.1:1"));
    assert_eq!(output.exit(), Exit::Failed);
    assert!(output.err().contains("cannot reach the controller"));
}

#[test]
fn diff_exits_zero_when_the_hosts_run_what_the_checkout_declares() {
    let root = repo("diff-same", &[("hosts/h/web/compose.yaml", COMPOSE)]);
    let url = controller_with(StackState::Running);
    let output = run(&words(&format!(
        "diff {} --controller {url}",
        root.display()
    )));
    assert_eq!(output.exit(), Exit::Clean);
    assert!(output.out().contains("hosts/h/web  same"));
}

#[test]
fn diff_exits_one_when_a_declared_stack_is_not_running() {
    let root = repo("diff-failed", &[("hosts/h/web/compose.yaml", COMPOSE)]);
    let url = controller_with(StackState::Failed);
    let output = run(&words(&format!(
        "diff {} --controller {url}",
        root.display()
    )));
    assert_eq!(output.exit(), Exit::Found);
}

#[test]
fn diff_exits_two_before_calling_the_controller_when_the_path_is_wrong() {
    let line = "diff /nonexistent/aegis-checkout --controller http://127.0.0.1:1";
    let output = run(&words(line));
    assert_eq!(output.exit(), Exit::Failed);
    assert!(output.err().contains("is not a directory"));
}

#[test]
fn sync_exits_zero_when_every_stack_runs_after_it() {
    let url = controller_with(StackState::Running);
    let output = run(&words(&format!("sync --controller {url}")));
    assert_eq!(output.exit(), Exit::Clean);
    assert!(output.out().contains("h: at abc1234"));
}

#[test]
fn sync_exits_one_when_a_stack_failed_to_deploy() {
    let url = controller_with(StackState::Failed);
    let output = run(&words(&format!("sync --controller {url}")));
    assert_eq!(output.exit(), Exit::Found);
}

#[test]
fn sync_exits_two_when_the_controller_has_no_commit_to_sync_to() {
    let routes = Router::new().route(SYNCS_PATH, post(|| async { StatusCode::CONFLICT }));
    let url = serve(routes);
    let output = run(&words(&format!("sync --controller {url}")));
    assert_eq!(output.exit(), Exit::Failed);
    assert!(output.err().contains("409"));
}

#[test]
fn sync_exits_two_for_a_bad_host_before_calling_anything() {
    let output = run(&words("sync --host ../x --controller http://127.0.0.1:1"));
    assert_eq!(output.exit(), Exit::Failed);
    assert!(output.err().contains("--host"));
}
