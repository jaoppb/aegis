//! The bootstrap use case against in-memory fakes of every port.

use std::future::{Future, ready};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use aegis_application::{
    AgeRecipient, AuthKey, Bootstrap, BootstrapError, BootstrapPlan, ControllerChoice,
    ControllerUrl, Enrollment, Existing, FileText, HostFiles, HostKey, HostSide, ImageTag,
    KeyMinter, NodeKeys, Reason, RenderedFile, RepoUrl, SecretEncryptor, SidecarState, Sources,
    StackRunner, StateVolume, TailnetError, Written,
};
use aegis_domain::{AegisRole, HostName, HostPath, Secret, StackName, TailnetTags};

#[cfg(test)]
const RECIPIENT: &str = "age1ql3z7hjy54pw3hyww5ayyfg7zqgvc7w3j2elw8zmrj2kg5sfn9aqmcac8p";
#[cfg(test)]
const AUTH_KEY: &str = "tskey-auth-single-use";
#[cfg(test)]
const OAUTH_SECRET: &str = "tskey-client-oauth-secret";

#[cfg(test)]
#[derive(Default)]
struct Files {
    written: Mutex<Vec<RenderedFile>>,
    existing: Mutex<Vec<Existing>>,
    outcome: Option<Written>,
    checkout_error: Option<BootstrapError>,
}

#[cfg(test)]
impl Files {
    fn unchanged() -> Self {
        Self {
            outcome: Some(Written::Unchanged),
            ..Self::default()
        }
    }

    fn text_of(&self, path: &str) -> String {
        let written = self.written.lock().unwrap().clone();
        let file = written.iter().find(|file| file.path().as_str() == path);
        file.unwrap().text().as_str().to_owned()
    }

    fn paths(&self) -> Vec<String> {
        let written = self.written.lock().unwrap();
        written.iter().map(|file| file.path().to_string()).collect()
    }
}

#[cfg(test)]
impl HostFiles for &Files {
    fn ensure_age_key(&self) -> impl Future<Output = Result<HostKey, BootstrapError>> + Send {
        let key = HostKey::new(
            AgeRecipient::new(RECIPIENT).unwrap(),
            HostPath::new("/root/.config/sops/age/keys.txt").unwrap(),
        );
        ready(Ok(key))
    }

    fn ensure_checkout(
        &self,
        _repo: &RepoUrl,
    ) -> impl Future<Output = Result<(), BootstrapError>> + Send {
        ready(self.checkout_error.clone().map_or(Ok(()), Err))
    }

    fn write(
        &self,
        file: &RenderedFile,
        existing: Existing,
    ) -> impl Future<Output = Result<Written, BootstrapError>> + Send {
        self.written.lock().unwrap().push(file.clone());
        self.existing.lock().unwrap().push(existing);
        ready(Ok(self.outcome.unwrap_or(Written::Changed)))
    }
}

#[cfg(test)]
struct Cipher;

#[cfg(test)]
impl SecretEncryptor for Cipher {
    fn encrypt(
        &self,
        plaintext: &Secret,
        recipient: &AgeRecipient,
    ) -> impl Future<Output = Result<FileText, BootstrapError>> + Send {
        let text = format!("ENC[{recipient}:{}]", plaintext.expose().len());
        ready(Ok(FileText::new(text)))
    }
}

#[cfg(test)]
struct Keys(Option<&'static str>);

#[cfg(test)]
struct PerRole;

#[cfg(test)]
impl NodeKeys for PerRole {
    fn key_for(
        &self,
        _node: &HostName,
        role: AegisRole,
    ) -> impl Future<Output = Result<Option<AuthKey>, BootstrapError>> + Send {
        ready(Ok(Some(AuthKey::new(format!("key-for-{role:?}")))))
    }
}

#[cfg(test)]
impl NodeKeys for Keys {
    fn key_for(
        &self,
        _node: &HostName,
        _role: AegisRole,
    ) -> impl Future<Output = Result<Option<AuthKey>, BootstrapError>> + Send {
        ready(Ok(self.0.map(AuthKey::new)))
    }
}

#[cfg(test)]
#[derive(Default)]
struct Runner {
    started: Mutex<Vec<String>>,
    keys: Mutex<Vec<(String, Option<String>)>>,
    failing: Option<&'static str>,
}

#[cfg(test)]
impl StackRunner for &Runner {
    fn start(
        &self,
        stack: &StackName,
        key: Option<&AuthKey>,
    ) -> impl Future<Output = Result<(), BootstrapError>> + Send {
        self.started.lock().unwrap().push(stack.to_string());
        let key = key.map(|key| key.expose().to_owned());
        self.keys.lock().unwrap().push((stack.to_string(), key));
        let broken = self.failing == Some(stack.as_str());
        ready(
            broken
                .then(|| BootstrapError::Run(Reason::new(&"boom")))
                .map_or(Ok(()), Err),
        )
    }
}

#[cfg(test)]
fn block_on<Output>(future: impl Future<Output = Output>) -> Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(future)
}

#[cfg(test)]
fn plan(controller: ControllerChoice) -> BootstrapPlan {
    let sources = Sources::new(
        RepoUrl::new("git@github.com:me/infra.git").unwrap(),
        ControllerUrl::new("http://jao-server-aegis-controller:7480").unwrap(),
        HostPath::new("/var/lib/aegis/repo").unwrap(),
    );
    let plan = BootstrapPlan::new(
        HostName::new("jao-server").unwrap(),
        sources,
        ImageTag::new("0.1.0").unwrap(),
    );
    plan.with_controller(controller)
}

#[cfg(test)]
fn install() -> ControllerChoice {
    ControllerChoice::Install(Secret::new(format!(
        "TS_OAUTH_CLIENT_ID=id\nTS_OAUTH_CLIENT_SECRET={OAUTH_SECRET}\n"
    )))
}

#[cfg(test)]
struct Scenario {
    key: Option<&'static str>,
    choice: ControllerChoice,
}

#[cfg(test)]
impl Scenario {
    const fn new(key: Option<&'static str>, choice: ControllerChoice) -> Self {
        Self { key, choice }
    }
}

#[cfg(test)]
fn try_run(
    files: &Files,
    runner: &Runner,
    scenario: Scenario,
) -> Result<aegis_application::BootstrapReport, BootstrapError> {
    let host = HostSide::new(files, Cipher);
    let bootstrap = Bootstrap::new(host, Keys(scenario.key), runner);
    block_on(bootstrap.run(&plan(scenario.choice)))
}

#[cfg(test)]
fn run(files: &Files, runner: &Runner, scenario: Scenario) -> aegis_application::BootstrapReport {
    try_run(files, runner, scenario).unwrap()
}

#[cfg(test)]
mod cases {
    use super::*;

    #[test]
    fn the_agent_stack_runs_the_tagged_image_with_the_reserved_name() {
        let (files, runner) = (Files::default(), Runner::default());
        run(
            &files,
            &runner,
            Scenario::new(Some(AUTH_KEY), ControllerChoice::Skip),
        );
        let compose = files.text_of("aegis/agent/compose.yaml");
        assert!(compose.contains("image: \"ghcr.io/jaoppb/aegis-agent:0.1.0\""));
        assert!(compose.contains("AEGIS_RESERVED: \"agent\""));
        assert!(compose.contains("AEGIS_HOST: \"jao-server\""));
        assert!(compose.contains("TS_HOSTNAME: \"jao-server-aegis-agent\""));
        assert_eq!(*runner.started.lock().unwrap(), ["agent"]);
    }

    #[test]
    fn the_single_use_key_goes_to_compose_and_never_into_a_file() {
        let (files, runner) = (Files::default(), Runner::default());
        run(
            &files,
            &runner,
            Scenario::new(Some(AUTH_KEY), ControllerChoice::Skip),
        );
        let compose = files.text_of("aegis/agent/compose.yaml");
        assert!(!compose.contains(AUTH_KEY));
        assert!(compose.contains("${TS_AUTHKEY:-}"));
        assert_eq!(
            *runner.keys.lock().unwrap(),
            [("agent".to_owned(), Some(AUTH_KEY.to_owned()))]
        );
    }

    #[test]
    fn a_rerun_with_an_enrolled_sidecar_mints_nothing_and_reports_nothing_to_do() {
        let (files, runner) = (Files::unchanged(), Runner::default());
        let report = run(&files, &runner, Scenario::new(None, ControllerChoice::Skip));
        assert_eq!(*runner.keys.lock().unwrap(), [("agent".to_owned(), None)]);
        assert!(report.is_noop());
        assert!(report.to_string().contains("nothing to do"));
        assert!(report.to_string().contains(RECIPIENT));
    }

    #[test]
    fn a_first_run_is_not_a_noop() {
        let (files, runner) = (Files::default(), Runner::default());
        let report = run(
            &files,
            &runner,
            Scenario::new(Some(AUTH_KEY), ControllerChoice::Skip),
        );
        assert!(!report.is_noop());
        assert!(report.to_string().contains("joined the tailnet"));
    }

    #[test]
    fn the_controller_starts_first_and_its_credentials_are_written_encrypted_once() {
        let (files, runner) = (Files::default(), Runner::default());
        run(&files, &runner, Scenario::new(Some(AUTH_KEY), install()));
        assert_eq!(*runner.started.lock().unwrap(), ["controller", "agent"]);
        let env = files.text_of("aegis/controller/controller.env");
        assert!(env.starts_with("ENC["));
        assert!(!env.contains(OAUTH_SECRET));
        assert_eq!(
            *files.existing.lock().unwrap(),
            [Existing::Replace, Existing::Keep, Existing::Replace]
        );
        let agent = files.text_of("aegis/agent/compose.yaml");
        assert!(agent.contains("AEGIS_RESERVED: \"agent,controller\""));
    }

    #[test]
    fn no_file_holds_the_oauth_secret_in_plaintext() {
        let (files, runner) = (Files::default(), Runner::default());
        run(&files, &runner, Scenario::new(Some(AUTH_KEY), install()));
        let leaked = files
            .paths()
            .into_iter()
            .filter(|path| files.text_of(path).contains(OAUTH_SECRET));
        assert_eq!(leaked.count(), 0);
    }

    #[test]
    fn without_the_controller_no_controller_file_is_written() {
        let (files, runner) = (Files::default(), Runner::default());
        run(&files, &runner, Scenario::new(None, ControllerChoice::Skip));
        assert_eq!(files.paths(), ["aegis/agent/compose.yaml"]);
    }

    #[test]
    fn a_recipient_must_be_an_age_public_key() {
        assert!(AgeRecipient::new(RECIPIENT).is_ok());
        assert!(AgeRecipient::new("age1").is_err());
        assert!(AgeRecipient::new("AGE-SECRET-KEY-1ABC").is_err());
        assert!(AgeRecipient::new("age1abc\nevil").is_err());
    }

    #[test]
    fn an_image_tag_cannot_break_out_of_a_scalar() {
        assert!(ImageTag::new("0.1.0-rc.1").is_ok());
        assert!(ImageTag::new("").is_err());
        assert!(ImageTag::new("-x").is_err());
        assert!(ImageTag::new("a\"b").is_err());
        assert!(ImageTag::new("a b").is_err());
    }

    #[test]
    fn a_controller_url_is_http_without_quotes_or_interpolation() {
        assert!(ControllerUrl::new("http://controller:7480").is_ok());
        assert!(ControllerUrl::new("ftp://controller").is_err());
        assert!(ControllerUrl::new("http://a\"b").is_err());
        assert!(ControllerUrl::new("http://a/$HOME").is_err());
    }

    #[test]
    fn a_repository_url_never_starts_like_a_git_option() {
        assert!(RepoUrl::new("git@github.com:me/infra.git").is_ok());
        assert!(RepoUrl::new("--upload-pack=evil").is_err());
        assert!(RepoUrl::new("a b").is_err());
        assert_eq!(
            format!("{:?}", RepoUrl::new("ssh://git@host/r").unwrap()),
            "RepoUrl(<redacted>)"
        );
    }

    #[test]
    fn a_failed_checkout_writes_nothing_and_starts_nothing() {
        let failure = BootstrapError::Checkout(Reason::new(&"git exited with 128"));
        let files = Files {
            checkout_error: Some(failure.clone()),
            ..Files::default()
        };
        let runner = Runner::default();
        let result = try_run(&files, &runner, Scenario::new(Some(AUTH_KEY), install()));
        assert_eq!(result.unwrap_err(), failure);
        assert!(files.paths().is_empty());
        assert!(runner.started.lock().unwrap().is_empty());
    }

    #[test]
    fn a_failing_controller_start_leaves_the_agent_unstarted() {
        let files = Files::default();
        let runner = Runner {
            failing: Some("controller"),
            ..Runner::default()
        };
        let result = try_run(&files, &runner, Scenario::new(Some(AUTH_KEY), install()));
        assert!(matches!(result, Err(BootstrapError::Run(_))));
        assert_eq!(*runner.started.lock().unwrap(), ["controller"]);
    }

    #[test]
    fn each_stack_gets_the_key_of_its_own_role() {
        let (files, runner) = (Files::default(), Runner::default());
        let host = HostSide::new(&files, Cipher);
        let bootstrap = Bootstrap::new(host, PerRole, &runner);
        block_on(bootstrap.run(&plan(install()))).unwrap();
        assert_eq!(
            *runner.keys.lock().unwrap(),
            [
                (
                    "controller".to_owned(),
                    Some("key-for-Controller".to_owned())
                ),
                ("agent".to_owned(), Some("key-for-Agent".to_owned())),
            ]
        );
    }

    #[test]
    fn a_repository_url_with_a_password_is_refused() {
        let refused = RepoUrl::new("https://user:token@host/r.git");
        assert_eq!(
            refused.unwrap_err(),
            aegis_domain::ValueError::EmbeddedPassword
        );
        assert!(RepoUrl::new("ssh://git@github.com/me/infra.git").is_ok());
        assert!(RepoUrl::new("https://github.com/me/infra.git").is_ok());
    }

    #[test]
    fn the_rendered_sidecar_uses_kernel_networking_and_waits_to_be_healthy() {
        let (files, runner) = (Files::default(), Runner::default());
        run(
            &files,
            &runner,
            Scenario::new(Some(AUTH_KEY), ControllerChoice::Skip),
        );
        let compose = files.text_of("aegis/agent/compose.yaml");
        assert!(compose.contains("TS_USERSPACE: \"false\""));
        assert!(compose.contains("condition: service_healthy"));
        assert!(compose.contains("healthcheck:"));
    }
}

#[cfg(test)]
struct Volume(StateVolume);

#[cfg(test)]
impl SidecarState for Volume {
    fn volume(&self, _node: &HostName) -> impl Future<Output = Result<StateVolume, Reason>> + Send {
        ready(Ok(self.0))
    }
}

#[cfg(test)]
struct Broken(Result<StateVolume, Reason>);

#[cfg(test)]
impl SidecarState for Broken {
    fn volume(&self, _node: &HostName) -> impl Future<Output = Result<StateVolume, Reason>> + Send {
        ready(self.0.clone())
    }
}

#[cfg(test)]
struct Refusing;

#[cfg(test)]
impl KeyMinter for Refusing {
    fn mint(
        &self,
        _tags: &TailnetTags,
    ) -> impl Future<Output = Result<AuthKey, TailnetError>> + Send {
        ready(Err(TailnetError::Rejected(Reason::new(&"403"))))
    }
}

#[cfg(test)]
#[derive(Default)]
struct Minter {
    tags: Mutex<Vec<String>>,
    mints: AtomicUsize,
}

#[cfg(test)]
impl KeyMinter for &Minter {
    fn mint(
        &self,
        tags: &TailnetTags,
    ) -> impl Future<Output = Result<AuthKey, TailnetError>> + Send {
        self.mints.fetch_add(1, Ordering::SeqCst);
        let names = tags.iter().map(ToString::to_string);
        self.tags.lock().unwrap().extend(names);
        ready(Ok(AuthKey::new(AUTH_KEY)))
    }
}

#[cfg(test)]
mod joining {
    use super::*;

    fn node() -> HostName {
        HostName::new("jao-server-aegis-agent").unwrap()
    }

    #[test]
    fn a_populated_sidecar_mints_nothing() {
        let minter = Minter::default();
        let joiner = Enrollment::new(Volume(StateVolume::Populated), &minter);
        let key = block_on(joiner.key_for(&node(), AegisRole::Agent)).unwrap();
        assert!(key.is_none());
        assert_eq!(minter.mints.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn an_agent_key_is_tagged_as_an_agent() {
        let minter = Minter::default();
        let joiner = Enrollment::new(Volume(StateVolume::Empty), &minter);
        block_on(joiner.key_for(&node(), AegisRole::Agent)).unwrap();
        assert_eq!(*minter.tags.lock().unwrap(), ["tag:aegis-agent"]);
    }

    #[test]
    fn an_unreadable_state_mints_nothing_and_says_so() {
        let minter = Minter::default();
        let broken = Err(Reason::new(&"docker down"));
        let joiner = Enrollment::new(Broken(broken), &minter);
        let failure = block_on(joiner.key_for(&node(), AegisRole::Agent)).unwrap_err();
        assert!(matches!(failure, BootstrapError::State(_)));
        assert_eq!(minter.mints.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_refused_mint_is_reported_as_a_mint_failure() {
        let joiner = Enrollment::new(Volume(StateVolume::Empty), Refusing);
        let failure = block_on(joiner.key_for(&node(), AegisRole::Agent)).unwrap_err();
        assert!(matches!(failure, BootstrapError::Mint(_)));
    }

    #[test]
    fn an_empty_sidecar_mints_one_key_tagged_with_its_role() {
        let minter = Minter::default();
        let joiner = Enrollment::new(Volume(StateVolume::Empty), &minter);
        let key = block_on(joiner.key_for(&node(), AegisRole::Controller)).unwrap();
        assert!(key.is_some());
        assert_eq!(*minter.tags.lock().unwrap(), ["tag:aegis-controller"]);
    }
}
