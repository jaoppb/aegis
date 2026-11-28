//! `aegis bootstrap`: turn the flags and the environment into a plan and run it on the local
//! host.

use std::path::PathBuf;

use aegis_adapters::bootstrap::{
    ComposeStacks, CredentialsError, DockerVolumes, Layout, LocalHost, OAuthEnvironment,
    SopsEncrypt,
};
use aegis_adapters::tailscale::TailscaleApi;
use aegis_application::{
    Bootstrap, BootstrapError, BootstrapPlan, BootstrapReport, ControllerChoice, ControllerUrl,
    HostSide, ImageTag, Joiner, Reason, RepoUrl, Sources, TailnetError,
};
use aegis_domain::{HostName, HostPath, ValueError};

use crate::args::{Flags, UsageError};

/// Where the repository is cloned, unless `--checkout` says otherwise.
const DEFAULT_CHECKOUT: &str = "/var/lib/aegis/repo";

/// Where the stack files are rendered, unless `--stacks` says otherwise.
const DEFAULT_STACKS: &str = "/var/lib/aegis/stacks";

/// The tag of the images: the version of this binary, unless `--image-tag` says otherwise.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Something went wrong while bootstrapping.
#[derive(Debug, thiserror::Error)]
pub enum BootstrapFailure {
    /// A flag is missing or invalid.
    #[error("{0}")]
    Usage(#[from] UsageError),
    /// The OAuth credentials are missing or unusable.
    #[error("{0}")]
    Credentials(#[from] CredentialsError),
    /// The home directory is unknown, so the age key has no place.
    #[error("{0}")]
    Home(Reason),
    /// The Tailscale client could not be built.
    #[error("{0}")]
    Tailnet(TailnetError),
    /// A step of the bootstrap failed.
    #[error("{0}")]
    Step(#[from] BootstrapError),
}

fn parsed<Value>(
    flag: &'static str,
    result: Result<Value, ValueError>,
) -> Result<Value, UsageError> {
    result.map_err(|cause| UsageError::Invalid { flag, cause })
}

fn path_of(flags: &Flags, flag: &'static str, default: &str) -> Result<HostPath, UsageError> {
    let text = flags.value(flag).unwrap_or(default);
    parsed(flag, HostPath::new(text))
}

fn sources(flags: &Flags) -> Result<Sources, UsageError> {
    let repo = parsed("--repo", RepoUrl::new(flags.required("--repo")?))?;
    let controller = flags.required("--controller")?;
    let controller = parsed("--controller", ControllerUrl::new(controller))?;
    let checkout = path_of(flags, "--checkout", DEFAULT_CHECKOUT)?;
    Ok(Sources::new(repo, controller, checkout))
}

fn choice(flags: &Flags, oauth: &OAuthEnvironment) -> ControllerChoice {
    let install = flags.has("--with-controller");
    let credentials = install.then(|| oauth.dotenv().clone());
    credentials.map_or(ControllerChoice::Skip, ControllerChoice::Install)
}

/// The plan the flags and the credentials ask for.
///
/// # Errors
///
/// Returns a [`UsageError`] for a missing or invalid flag.
pub fn plan(flags: &Flags, oauth: &OAuthEnvironment) -> Result<BootstrapPlan, UsageError> {
    let host = parsed("--host", HostName::new(flags.required("--host")?))?;
    let tag = flags.value("--image-tag").unwrap_or(VERSION);
    let image = parsed("--image-tag", ImageTag::new(tag))?;
    let plan = BootstrapPlan::new(host, sources(flags)?, image);
    Ok(plan.with_controller(choice(flags, oauth)))
}

/// Run the bootstrap the flags ask for on this host.
///
/// # Errors
///
/// Returns a [`BootstrapFailure`] for a bad flag, missing credentials or a failed step.
pub async fn run(flags: &Flags) -> Result<BootstrapReport, BootstrapFailure> {
    let oauth = OAuthEnvironment::from_environment()?;
    let plan = plan(flags, &oauth)?;
    let stacks = PathBuf::from(path_of(flags, "--stacks", DEFAULT_STACKS)?.as_str());
    let checkout = PathBuf::from(plan.sources().checkout().as_str());
    let key = Layout::default_key().map_err(BootstrapFailure::Home)?;
    let api = TailscaleApi::new(oauth.credentials().clone()).map_err(BootstrapFailure::Tailnet)?;
    let host = HostSide::new(
        LocalHost::new(Layout::new(checkout, stacks.clone(), key)),
        SopsEncrypt,
    );
    let keys = Joiner::new(DockerVolumes::new(), api);
    let bootstrap = Bootstrap::new(host, keys, ComposeStacks::new(stacks));
    Ok(bootstrap.run(&plan).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{Invocation, parse};

    fn flags(line: &str) -> Flags {
        let words: Vec<String> = line.split_whitespace().map(str::to_owned).collect();
        match parse(&words) {
            Ok(Invocation::Bootstrap(flags)) => flags,
            other => panic!("not a bootstrap: {other:?}"),
        }
    }

    fn oauth() -> OAuthEnvironment {
        let client = Some("abc".to_owned());
        OAuthEnvironment::from_values(client, Some("def".to_owned())).unwrap()
    }

    const REQUIRED: &str =
        "bootstrap --repo git@host:r.git --host jao-server --controller http://c:7480";

    #[test]
    fn the_defaults_fill_what_the_flags_leave_out() {
        let plan = plan(&flags(REQUIRED), &oauth()).unwrap();
        assert_eq!(plan.sources().checkout().as_str(), DEFAULT_CHECKOUT);
        assert_eq!(plan.image().as_str(), VERSION);
        assert_eq!(plan.controller(), &ControllerChoice::Skip);
    }

    #[test]
    fn with_controller_carries_the_credentials_to_encrypt() {
        let line = format!("{REQUIRED} --with-controller");
        let plan = plan(&flags(&line), &oauth()).unwrap();
        assert_eq!(
            plan.controller(),
            &ControllerChoice::Install(oauth().dotenv().clone())
        );
    }

    #[test]
    fn a_missing_required_flag_is_named() {
        let line = "bootstrap --host jao-server --controller http://c:7480";
        assert_eq!(
            plan(&flags(line), &oauth()).unwrap_err(),
            UsageError::Required("--repo")
        );
    }

    #[test]
    fn an_invalid_value_names_its_flag() {
        let line = "bootstrap --repo r --host bad/host --controller http://c:7480";
        let error = plan(&flags(line), &oauth()).unwrap_err();
        assert!(matches!(error, UsageError::Invalid { flag: "--host", .. }));
        let relative = format!("{REQUIRED} --checkout relative/dir");
        let error = plan(&flags(&relative), &oauth()).unwrap_err();
        assert!(matches!(
            error,
            UsageError::Invalid {
                flag: "--checkout",
                ..
            }
        ));
    }
}
