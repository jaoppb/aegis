//! `aegis bootstrap`: turn the flags and the environment into a plan and run it on the local
//! host.

use std::path::PathBuf;

use aegis_adapters::DockerStateProbe;
use aegis_adapters::bootstrap::{
    ComposeStacks, CredentialsError, Layout, LocalHost, OAuthEnvironment, SopsEncrypt,
};
use aegis_adapters::tailscale::TailscaleApi;
use aegis_application::{
    AegisNodes, Bootstrap, BootstrapError, BootstrapPlan, BootstrapReport, ControllerChoice,
    ControllerSetup, ControllerUrl, Enrollment, HostSide, ImageTag, Reason, RepoUrl, Sources,
    TailnetError,
};
use aegis_domain::{AegisRole, HostName, HostPath, TailnetTags, ValueError};

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

/// The tags of `--enroll-tags`, comma-separated: the only tags a stack's node may carry, so none
/// may be an Aegis role tag.
fn enroll_tags(flags: &Flags) -> Result<TailnetTags, UsageError> {
    let listed = flags.required("--enroll-tags")?;
    let tags = parsed("--enroll-tags", TailnetTags::parse_list(listed))?;
    let role = tags.iter().find(|tag| AegisRole::is_role_tag(tag));
    if let Some(reserved) = role {
        return Err(UsageError::ReservedTag(reserved.to_string()));
    }
    if tags.is_empty() {
        return Err(UsageError::Required("--enroll-tags"));
    }
    Ok(tags)
}

fn choice(flags: &Flags, oauth: &OAuthEnvironment) -> Result<ControllerChoice, UsageError> {
    if !flags.has("--with-controller") {
        return Ok(ControllerChoice::Skip);
    }
    let setup = ControllerSetup::new(oauth.dotenv().clone(), enroll_tags(flags)?);
    Ok(ControllerChoice::Install(setup))
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
    Ok(plan.with_controller(choice(flags, oauth)?))
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
    let keys = AegisNodes::new(DockerStateProbe::default(), Enrollment::new(api));
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
        let line = format!("{REQUIRED} --with-controller --enroll-tags tag:app,tag:db");
        let plan = plan(&flags(&line), &oauth()).unwrap();
        let setup = match plan.controller() {
            ControllerChoice::Install(setup) => setup,
            ControllerChoice::Skip => panic!("the controller is not installed"),
        };
        assert_eq!(setup.credentials(), oauth().dotenv());
        assert_eq!(setup.enroll_tags(), "tag:app,tag:db");
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

    #[test]
    fn the_overrides_replace_the_defaults() {
        let line =
            format!("{REQUIRED} --image-tag 9.9.9 --checkout /srv/repo --stacks /srv/stacks");
        let plan = plan(&flags(&line), &oauth()).unwrap();
        assert_eq!(plan.image().as_str(), "9.9.9");
        assert_eq!(plan.sources().checkout().as_str(), "/srv/repo");
        let stacks = path_of(&flags(&line), "--stacks", DEFAULT_STACKS).unwrap();
        assert_eq!(stacks.as_str(), "/srv/stacks");
    }

    #[test]
    fn a_flag_of_another_command_is_not_accepted_by_bootstrap() {
        let words: Vec<String> = ["bootstrap", "--force"].map(str::to_owned).to_vec();
        assert!(parse(&words).is_err());
    }

    #[test]
    fn the_controller_needs_the_tags_its_stacks_may_carry() {
        let line = format!("{REQUIRED} --with-controller");
        let error = plan(&flags(&line), &oauth()).unwrap_err();
        assert_eq!(error, UsageError::Required("--enroll-tags"));
    }

    #[test]
    fn a_stack_tag_cannot_be_an_aegis_role_tag() {
        let line = format!("{REQUIRED} --with-controller --enroll-tags tag:app,tag:aegis-agent");
        let error = plan(&flags(&line), &oauth()).unwrap_err();
        assert_eq!(error, UsageError::ReservedTag("tag:aegis-agent".to_owned()));
    }

    #[test]
    fn enroll_tags_are_not_needed_without_the_controller() {
        assert!(plan(&flags(REQUIRED), &oauth()).is_ok());
    }

    #[test]
    fn blank_enroll_tags_count_as_none() {
        let line = format!("{REQUIRED} --with-controller --enroll-tags ,");
        let error = plan(&flags(&line), &oauth()).unwrap_err();
        assert_eq!(error, UsageError::Required("--enroll-tags"));
    }
}
