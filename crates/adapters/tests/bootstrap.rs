//! The bootstrap adapters: real `age-keygen`, `sops` and `git` where the machine has them, a fake
//! `docker` that records how it was run, and the rendered stacks checked by the real validator.

use std::future::{Future, ready};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use aegis_adapters::bootstrap::{ComposeStacks, Layout, LocalHost, SopsEncrypt};
use aegis_adapters::{FsRepo, SerdeStackParser, SopsCheck};
use aegis_application::{
    AuthKey, Bootstrap, BootstrapError, BootstrapPlan, ControllerChoice, ControllerSetup,
    ControllerUrl, Existing, FileText, HostFiles, HostKey, HostSide, ImageTag, NodeKeys,
    RenderedFile, RepoUrl, SecretEncryptor, Sources, StackRunner, Written, load_desired_state,
    validate_desired_state,
};
use aegis_domain::{
    AegisRole, HostName, HostPath, ProjectName, RepoPath, Secret, StackName, TailnetTag,
    TailnetTags,
};

#[cfg(test)]
fn block_on<Output>(future: impl Future<Output = Output>) -> Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(future)
}

#[cfg(test)]
fn read(path: &Path) -> String {
    block_on(tokio::fs::read_to_string(path)).unwrap()
}

#[cfg(test)]
fn mode_of(path: &Path) -> u32 {
    let metadata = block_on(tokio::fs::metadata(path)).unwrap();
    metadata.permissions().mode()
}

/// A scratch directory that starts empty.
#[cfg(test)]
fn scratch(name: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!(
        "aegis-bootstrap-{}-{unique}-{name}",
        std::process::id()
    ));
    block_on(tokio::fs::remove_dir_all(&root)).ok();
    block_on(tokio::fs::create_dir_all(&root)).unwrap();
    root
}

#[cfg(test)]
fn host_in(root: &Path) -> LocalHost {
    let layout = Layout::new(
        root.join("repo"),
        root.join("stacks"),
        root.join("config/sops/age/keys.txt"),
    );
    LocalHost::new(layout)
}

#[cfg(test)]
fn file(path: &str, text: &str) -> RenderedFile {
    RenderedFile::new(RepoPath::new(path).unwrap(), FileText::new(text))
}

#[cfg(test)]
mod files {
    use super::*;

    #[test]
    fn the_age_key_is_generated_once_and_its_recipient_is_stable() {
        let root = scratch("age");
        let host = host_in(&root);
        let first = block_on(host.ensure_age_key()).unwrap();
        let second = block_on(host.ensure_age_key()).unwrap();
        assert_eq!(first, second);
        assert!(first.recipient().as_str().starts_with("age1"));
        let key = root.join("config/sops/age/keys.txt");
        let mode = mode_of(&key);
        assert_eq!(mode & 0o777, 0o600);
        let directory = key.parent().unwrap();
        let mode = mode_of(directory);
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn a_foreign_key_file_is_never_overwritten_and_never_printed() {
        let root = scratch("garbage");
        let key = root.join("config/sops/age/keys.txt");
        block_on(tokio::fs::create_dir_all(key.parent().unwrap())).unwrap();
        block_on(tokio::fs::write(&key, "not-a-key super-secret-material")).unwrap();
        let failure = block_on(host_in(&root).ensure_age_key()).unwrap_err();
        assert!(matches!(failure, BootstrapError::AgeKey(_)));
        assert!(!failure.to_string().contains("super-secret"), "{failure}");
        assert_eq!(read(&key), "not-a-key super-secret-material");
    }

    #[test]
    fn a_file_is_replaced_only_when_its_text_changes() {
        let root = scratch("write");
        let host = host_in(&root);
        let compose = file("aegis/agent/compose.yaml", "a: 1\n");
        let first = block_on(host.write(&compose, Existing::Replace)).unwrap();
        let again = block_on(host.write(&compose, Existing::Replace)).unwrap();
        let changed = file("aegis/agent/compose.yaml", "a: 2\n");
        let third = block_on(host.write(&changed, Existing::Replace)).unwrap();
        assert_eq!(
            [first, again, third],
            [Written::Changed, Written::Unchanged, Written::Changed]
        );
        let text = read(&root.join("stacks/aegis/agent/compose.yaml"));
        assert_eq!(text, "a: 2\n");
    }

    #[test]
    fn a_kept_file_survives_a_different_text_and_is_private() {
        let root = scratch("keep");
        let host = host_in(&root);
        let first = file("aegis/controller/controller.env", "first");
        let second = file("aegis/controller/controller.env", "second");
        block_on(host.write(&first, Existing::Keep)).unwrap();
        let kept = block_on(host.write(&second, Existing::Keep)).unwrap();
        assert_eq!(kept, Written::Unchanged);
        let path = root.join("stacks/aegis/controller/controller.env");
        assert_eq!(read(&path), "first");
        let mode = mode_of(&path);
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn the_repository_is_cloned_then_fetched_and_never_pushed() {
        let root = scratch("git");
        let origin = root.join("origin");
        git(&root, &["init", "--quiet", "origin"]);
        git(
            &origin,
            &["commit", "--quiet", "--allow-empty", "-m", "one"],
        );
        let host = host_in(&root);
        let url = RepoUrl::new(origin.display().to_string()).unwrap();
        block_on(host.ensure_checkout(&url)).unwrap();
        assert!(root.join("repo/.git").exists());
        git(
            &origin,
            &["commit", "--quiet", "--allow-empty", "-m", "two"],
        );
        block_on(host.ensure_checkout(&url)).unwrap();
        let log = git(&root.join("repo"), &["log", "--all", "--oneline"]);
        assert_eq!(log.lines().count(), 2);
    }

    #[test]
    fn a_failing_clone_names_git_and_not_the_url() {
        let root = scratch("badclone");
        let host = host_in(&root);
        let url = RepoUrl::new("/nonexistent/secret-token-repo").unwrap();
        let failure = block_on(host.ensure_checkout(&url)).unwrap_err();
        assert!(!failure.to_string().contains("secret-token"), "{failure}");
    }
}

#[cfg(test)]
fn git(directory: &Path, arguments: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["-c", "commit.gpgsign=false", "-c", "user.name=t"])
        .args(["-c", "user.email=t@example.com", "-C"])
        .arg(directory)
        .args(arguments)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {arguments:?} failed");
    String::from_utf8(output.stdout).unwrap()
}

#[cfg(test)]
mod encryption {
    use super::*;

    #[test]
    fn the_credentials_are_encrypted_to_the_host_key_and_pass_the_sops_check() {
        let root = scratch("sops");
        let key = block_on(host_in(&root).ensure_age_key()).unwrap();
        let plain = Secret::new("TS_OAUTH_CLIENT_ID=abc\nTS_OAUTH_CLIENT_SECRET=hunter-two\n");
        let sealed = block_on(SopsEncrypt.encrypt(&plain, key.recipient())).unwrap();
        assert!(!sealed.as_str().contains("hunter-two"));
        assert!(sealed.as_str().contains(key.recipient().as_str()));
        let name = RepoPath::new("controller.env").unwrap();
        assert!(aegis_application::EncryptionCheck::is_encrypted(
            &SopsCheck,
            &name,
            sealed.as_str()
        ));
    }
}

#[cfg(test)]
struct Fake {
    log: PathBuf,
    program: PathBuf,
}

#[cfg(test)]
impl Fake {
    fn install(root: &Path, body: &str) -> Self {
        let log = root.join("docker.log");
        let script = format!(
            "#!/bin/sh\necho \"ARGS:$*\" >> {log}\necho \"KEY:${{AEGIS_TS_AUTHKEY-unset}}\" >> {log}\n{body}\n",
            log = log.display()
        );
        let program = root.join("fake-docker");
        block_on(tokio::fs::write(&program, script)).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self { log, program }
    }

    fn logged(&self) -> String {
        block_on(tokio::fs::read_to_string(&self.log)).unwrap_or_default()
    }

    fn program(&self) -> String {
        self.program.display().to_string()
    }
}

#[cfg(test)]
mod compose {
    use super::*;

    fn stacks(body: &str) -> (Fake, ComposeStacks) {
        let root = scratch("compose");
        let directory = root.join("stacks/aegis/agent");
        block_on(tokio::fs::create_dir_all(&directory)).unwrap();
        let text = "services:\n  agent:\n    image: \"x:1\"\n";
        block_on(tokio::fs::write(directory.join("compose.yaml"), text)).unwrap();
        let fake = Fake::install(&root, body);
        let runner = ComposeStacks::with_program(fake.program(), root.join("stacks"));
        (fake, runner)
    }

    fn agent() -> StackName {
        StackName::new("agent").unwrap()
    }

    #[test]
    fn the_key_reaches_compose_through_the_environment_only() {
        let (fake, runner) = stacks("exit 0");
        let key = AuthKey::new("tskey-auth-one");
        block_on(runner.start(&agent(), Some(&key))).unwrap();
        let logged = fake.logged();
        assert!(logged.contains("KEY:tskey-auth-one"));
        assert!(logged.contains("compose -p aegis-agent -f compose.yaml"));
        assert!(
            !logged.contains("ARGS:")
                || !logged
                    .lines()
                    .any(|line| line.starts_with("ARGS:") && line.contains("tskey"))
        );
    }

    #[test]
    fn without_a_key_compose_gets_no_key_variable() {
        let (fake, runner) = stacks("exit 0");
        block_on(runner.start(&agent(), None)).unwrap();
        assert!(fake.logged().contains("KEY:unset"));
    }

    #[test]
    fn a_compose_failure_never_leaks_the_key() {
        let (_fake, runner) = stacks("echo \"boom $AEGIS_TS_AUTHKEY\" >&2\nexit 1");
        let key = AuthKey::new("tskey-auth-leaky");
        let failure = block_on(runner.start(&agent(), Some(&key))).unwrap_err();
        assert!(
            !failure.to_string().contains("tskey-auth-leaky"),
            "{failure}"
        );
    }
}

/// A `HostFiles` that writes for real below a scratch directory but fakes the key and the clone.
#[cfg(test)]
struct Disk {
    host: LocalHost,
    key: HostKey,
}

#[cfg(test)]
impl HostFiles for Disk {
    fn ensure_age_key(&self) -> impl Future<Output = Result<HostKey, BootstrapError>> + Send {
        ready(Ok(self.key.clone()))
    }

    fn ensure_checkout(
        &self,
        _repo: &RepoUrl,
    ) -> impl Future<Output = Result<(), BootstrapError>> + Send {
        ready(Ok(()))
    }

    fn write(
        &self,
        file: &RenderedFile,
        existing: Existing,
    ) -> impl Future<Output = Result<Written, BootstrapError>> + Send {
        self.host.write(file, existing)
    }
}

#[cfg(test)]
struct Joined;

#[cfg(test)]
impl NodeKeys for Joined {
    fn key_for(
        &self,
        _project: &ProjectName,
        _role: AegisRole,
    ) -> impl Future<Output = Result<Option<AuthKey>, BootstrapError>> + Send {
        ready(Ok(None))
    }
}

#[cfg(test)]
#[derive(Default)]
struct Started(Mutex<Vec<String>>);

#[cfg(test)]
impl StackRunner for &Started {
    fn start(
        &self,
        stack: &StackName,
        _key: Option<&AuthKey>,
    ) -> impl Future<Output = Result<(), BootstrapError>> + Send {
        self.0.lock().unwrap().push(stack.to_string());
        ready(Ok(()))
    }
}

#[cfg(test)]
fn render_to_disk(root: &Path, choice: ControllerChoice) -> PathBuf {
    bootstrap_on_disk(root, choice);
    root.join("stacks")
}

#[cfg(test)]
fn bootstrap_on_disk(root: &Path, choice: ControllerChoice) -> aegis_application::BootstrapReport {
    bootstrap_with(root, choice, SopsEncrypt)
}

/// A cipher that "encrypts" to the plaintext, so a test can read what the controller will see.
#[cfg(test)]
struct Clear;

#[cfg(test)]
impl SecretEncryptor for Clear {
    fn encrypt(
        &self,
        plaintext: &Secret,
        _recipient: &aegis_application::AgeRecipient,
    ) -> impl Future<Output = Result<FileText, BootstrapError>> + Send {
        ready(Ok(FileText::new(plaintext.expose())))
    }
}

#[cfg(test)]
fn bootstrap_with<Cipher: SecretEncryptor + Sync>(
    root: &Path,
    choice: ControllerChoice,
    cipher: Cipher,
) -> aegis_application::BootstrapReport {
    let host = host_in(root);
    let key = block_on(host.ensure_age_key()).unwrap();
    let sources = Sources::new(
        RepoUrl::new("git@github.com:me/infra.git").unwrap(),
        ControllerUrl::new("http://jao-server-aegis-controller:7480").unwrap(),
        HostPath::new("/var/lib/aegis/repo").unwrap(),
    );
    let image = ImageTag::new("0.1.0").unwrap();
    let plan = BootstrapPlan::new(HostName::new("jao-server").unwrap(), sources, image);
    let disk = Disk { host, key };
    let started = Started::default();
    let bootstrap = Bootstrap::new(HostSide::new(disk, cipher), Joined, &started);
    block_on(bootstrap.run(&plan.with_controller(choice))).unwrap()
}

#[cfg(test)]
const OAUTH_DOTENV: &str = "AEGIS_TS_CLIENT_ID=a\nAEGIS_TS_CLIENT_SECRET=b\n";

#[cfg(test)]
fn controller() -> ControllerChoice {
    let tags = TailnetTags::new(vec![TailnetTag::new("tag:app").unwrap()]);
    let setup = ControllerSetup::new(Secret::new(OAUTH_DOTENV), tags);
    ControllerChoice::Install(setup)
}

#[cfg(test)]
fn supplies(text: &str, name: &str) -> bool {
    text.contains(&format!("{name}=")) || text.contains(&format!("{name}:"))
}

#[cfg(test)]
mod rendered {
    use super::*;

    fn validated(stacks: &Path) -> aegis_application::Outcome {
        let repo = FsRepo::new(stacks);
        let loaded = load_desired_state(&repo, &SerdeStackParser);
        validate_desired_state(&loaded, &repo, &SopsCheck)
    }

    fn rejected(outcome: &aegis_application::Outcome) -> Vec<String> {
        let rejected = outcome.rejected().iter();
        let found = rejected.flat_map(|rejection| rejection.violations().iter());
        found.map(ToString::to_string).collect()
    }

    #[test]
    fn the_rendered_agent_and_controller_stacks_pass_the_agents_own_validation() {
        let root = scratch("validate");
        let stacks = render_to_disk(&root, controller());
        let outcome = validated(&stacks);
        assert_eq!(rejected(&outcome), Vec::<String>::new());
        let names = outcome.state().reserved().iter();
        let names: Vec<String> = names.map(|stack| stack.name().to_string()).collect();
        assert_eq!(names, ["agent", "controller"]);
    }

    #[test]
    fn the_controller_stack_provides_every_variable_the_controller_requires() {
        let root = scratch("required");
        bootstrap_with(&root, controller(), Clear);
        let directory = root.join("stacks/aegis/controller");
        let provided = format!(
            "{}{}",
            read(&directory.join("compose.yaml")),
            read(&directory.join("controller.env"))
        );
        let required = aegis_adapters::api::CONTROLLER_REQUIRED
            .iter()
            .chain(aegis_adapters::api::REPOSITORY_REQUIRED.iter());
        let missing = required.filter(|name| !supplies(&provided, name));
        assert_eq!(missing.collect::<Vec<_>>(), Vec::<&&str>::new());
        assert!(provided.contains("AEGIS_LISTEN:"));
    }

    #[test]
    fn a_second_bootstrap_rewrites_nothing() {
        let root = scratch("twice");
        render_to_disk(&root, controller());
        let env = root.join("stacks/aegis/controller/controller.env");
        let before = read(&env);
        render_to_disk(&root, controller());
        assert_eq!(read(&env), before);
    }

    #[test]
    fn a_second_bootstrap_reports_nothing_to_do_and_keeps_every_file_byte_identical() {
        let root = scratch("noop");
        let first = bootstrap_on_disk(&root, controller());
        let compose = root.join("stacks/aegis/agent/compose.yaml");
        let before = read(&compose);
        let second = bootstrap_on_disk(&root, controller());
        assert!(!first.is_noop());
        assert!(second.is_noop(), "{second}");
        assert_eq!(read(&compose), before);
    }

    #[test]
    fn the_agent_alone_leaves_no_controller_directory() {
        let root = scratch("agent-only");
        let stacks = render_to_disk(&root, ControllerChoice::Skip);
        assert!(stacks.join("aegis/agent/compose.yaml").exists());
        assert!(!stacks.join("aegis/controller").exists());
    }
}
