//! The commands of an operator: `validate` on a checkout, and `status`, `diff` and `sync` against
//! the controller.

use std::future::Future;
use std::path::Path;

use aegis_adapters::BaseUrl;
use aegis_adapters::api::{ApiClient, ClientError};
use aegis_adapters::operator::{CheckoutError, Rendered, load_checkout};
use aegis_api::version_1::SyncRequest;
use aegis_application::ControllerUrl;
use aegis_domain::HostName;

use crate::args::{Flags, UsageError};

/// The environment variable that says where the controller is, when `--controller` does not.
pub const CONTROLLER_VAR: &str = "AEGIS_CONTROLLER";

/// A command could not complete.
#[derive(Debug, thiserror::Error)]
pub enum Failure {
    /// A flag is missing or invalid.
    #[error("{0}")]
    Usage(#[from] UsageError),
    /// The path is not a checkout of the repository.
    #[error("{0}")]
    Checkout(#[from] CheckoutError),
    /// The controller could not be called.
    #[error("{}", describe(.0))]
    Controller(ClientError),
    /// The async runtime could not start.
    #[error("cannot start the async runtime")]
    Runtime,
}

/// A readable account of a failed call, with what to do about the usual causes.
fn describe(error: &ClientError) -> String {
    match error {
        ClientError::Rejected(status) => refusal(status.as_u16()),
        ClientError::Transport(cause) if cause.is_timeout() => {
            "the controller did not answer in time".to_owned()
        }
        ClientError::Transport(cause) if cause.is_connect() => {
            "cannot reach the controller: is this machine on the tailnet?".to_owned()
        }
        ClientError::Transport(_) => "the call to the controller failed".to_owned(),
        ClientError::TooLarge | ClientError::Undecodable => {
            "the controller answered something this client cannot read".to_owned()
        }
    }
}

/// What a status the controller answered means to an operator.
fn refusal(status: u16) -> String {
    match status {
        403 => "the controller refused this machine: your Tailscale login must be listed in \
AEGIS_OPERATORS on the controller"
            .to_owned(),
        404 => "the controller answered 404: it has no agent for that host, or is an older version"
            .to_owned(),
        409 => {
            "the controller answered 409: no commit has loaded yet, or the newest commit cannot \
be read"
                .to_owned()
        }
        502 => "the controller answered 502: it cannot fetch the repository".to_owned(),
        503 => "the controller answered 503: it could not check this machine's identity".to_owned(),
        other => format!("the controller answered {other}"),
    }
}

/// The controller to call: `--controller`, or else `fallback` (the `AEGIS_CONTROLLER` variable).
fn controller_at(flags: &Flags, fallback: Option<String>) -> Result<BaseUrl, UsageError> {
    let given = flags.value("--controller").map(str::to_owned).or(fallback);
    let text = given.ok_or(UsageError::Required("--controller or AEGIS_CONTROLLER"))?;
    let url = ControllerUrl::new(text).map_err(|cause| UsageError::Invalid {
        flag: "--controller",
        cause,
    })?;
    Ok(BaseUrl::new(url.as_str()))
}

fn client(flags: &Flags) -> Result<ApiClient, Failure> {
    let base = controller_at(flags, std::env::var(CONTROLLER_VAR).ok())?;
    ApiClient::new(base).map_err(Failure::Controller)
}

fn block_on<Done>(call: impl Future<Output = Done>) -> Result<Done, Failure> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();
    runtime
        .map(|running| running.block_on(call))
        .map_err(|_| Failure::Runtime)
}

/// The path `validate` and `diff` read: the one given, or the current directory.
fn checkout_path(flags: &Flags) -> &Path {
    Path::new(flags.path().unwrap_or("."))
}

/// `aegis validate [path]`: offline validation of a checkout.
///
/// # Errors
///
/// Returns a [`Failure`] when `path` is not a checkout of the repository.
pub fn validate(flags: &Flags) -> Result<Rendered, Failure> {
    let outcome = load_checkout(checkout_path(flags))?;
    Ok(aegis_adapters::operator::validate(&outcome))
}

/// `aegis status`: the commit, the hosts and their stacks, as the controller reports them.
///
/// # Errors
///
/// Returns a [`Failure`] for a missing or invalid controller URL or a call that fails.
pub fn status(flags: &Flags) -> Result<Rendered, Failure> {
    let controller = client(flags)?;
    let status = block_on(controller.controller_status())?.map_err(Failure::Controller)?;
    Ok(aegis_adapters::operator::status(&status))
}

/// `aegis diff [path]`: the desired state of a checkout against what the hosts reported.
///
/// # Errors
///
/// Returns a [`Failure`] when `path` is not a checkout or the controller cannot be called.
pub fn diff(flags: &Flags) -> Result<Rendered, Failure> {
    let outcome = load_checkout(checkout_path(flags))?;
    let controller = client(flags)?;
    let status = block_on(controller.controller_status())?.map_err(Failure::Controller)?;
    Ok(aegis_adapters::operator::diff(&outcome, &status))
}

/// The host `--host` names, if it was given.
fn host_of(flags: &Flags) -> Result<Option<HostName>, UsageError> {
    let named = flags.value("--host").map(HostName::new);
    named.transpose().map_err(|cause| UsageError::Invalid {
        flag: "--host",
        cause,
    })
}

/// The request `--stack` asks for: one stack, or all of them.
fn request_of(flags: &Flags) -> Result<SyncRequest, UsageError> {
    let Some(stack) = flags.value("--stack") else {
        return Ok(SyncRequest::everything());
    };
    let valid = aegis_domain::StackName::new(stack);
    let stack = valid.map_err(|cause| UsageError::Invalid {
        flag: "--stack",
        cause,
    })?;
    Ok(SyncRequest::only(stack.as_str()))
}

/// `aegis sync [--host <name>] [--stack <name>]`: converge now, and say what the hosts did.
///
/// # Errors
///
/// Returns a [`Failure`] for an invalid flag or a call that fails.
pub fn sync(flags: &Flags) -> Result<Rendered, Failure> {
    let host = host_of(flags)?;
    let request = request_of(flags)?;
    let controller = client(flags)?;
    let answer = block_on(controller.sync(host.as_ref(), &request))?;
    let response = answer.map_err(Failure::Controller)?;
    Ok(aegis_adapters::operator::sync(&response))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{Invocation, parse};

    fn flags(line: &str) -> Flags {
        let words: Vec<String> = line.split_whitespace().map(str::to_owned).collect();
        match parse(&words) {
            Ok(
                Invocation::Status(flags)
                | Invocation::Sync(flags)
                | Invocation::Diff(flags)
                | Invocation::Validate(flags),
            ) => flags,
            other => panic!("not an operator command: {other:?}"),
        }
    }

    #[test]
    fn the_usual_refusals_say_what_to_do() {
        assert!(refusal(403).contains("AEGIS_OPERATORS"));
        assert!(refusal(409).contains("no commit has loaded"));
        assert_eq!(refusal(418), "the controller answered 418");
    }

    #[test]
    fn the_flag_wins_over_the_variable() {
        let flags = flags("status --controller http://flag:7480");
        let base = controller_at(&flags, Some("http://env:7480".to_owned())).unwrap();
        assert_eq!(base, BaseUrl::new("http://flag:7480"));
    }

    #[test]
    fn the_variable_is_the_fallback() {
        let base = controller_at(&flags("status"), Some("http://env:7480".to_owned())).unwrap();
        assert_eq!(base, BaseUrl::new("http://env:7480"));
    }

    #[test]
    fn no_controller_at_all_is_a_usage_error() {
        let error = controller_at(&flags("status"), None).unwrap_err();
        assert_eq!(
            error,
            UsageError::Required("--controller or AEGIS_CONTROLLER")
        );
    }

    #[test]
    fn a_controller_that_is_not_a_url_is_refused() {
        let error = controller_at(&flags("status --controller nope"), None).unwrap_err();
        assert!(matches!(
            error,
            UsageError::Invalid {
                flag: "--controller",
                ..
            }
        ));
    }

    #[test]
    fn a_host_and_a_stack_must_be_names() {
        assert!(host_of(&flags("sync --host ../x")).is_err());
        assert!(request_of(&flags("sync --stack ../x")).is_err());
        assert_eq!(host_of(&flags("sync")).unwrap(), None);
        assert_eq!(
            request_of(&flags("sync")).unwrap(),
            SyncRequest::everything()
        );
        let request = request_of(&flags("sync --stack web")).unwrap();
        assert_eq!(request, SyncRequest::only("web"));
    }
}
