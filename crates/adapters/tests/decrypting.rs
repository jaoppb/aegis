//! Per-host decryption around a compose runner: the real tmpfs store, a fake decryptor and a fake
//! runner that reads what Compose would see while it runs.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use aegis_adapters::{DecryptingCompose, SecretPorts, TmpfsStore};
use aegis_application::{
    BuiltServices, ComposeError, ComposeRunner, DecryptError, DeployRequest, Extras, Reason,
    SecretDecryptor, Target,
};
use aegis_domain::{
    CommitSha, ComposeSummary, EnvFile, EnvFiles, HostPath, Presence, ProjectName, RepoPath,
    Secret, SecretFile, SecretFiles, SecretName, Service, ServiceName, ServiceSource, Services,
    StackName, Volumes,
};

#[cfg(test)]
/// Decrypts from a table keyed by file name; a file not in the table does not exist.
struct FakeDecryptor {
    files: BTreeMap<&'static str, Result<&'static str, &'static str>>,
}

#[cfg(test)]
impl SecretDecryptor for FakeDecryptor {
    fn decrypt(
        &self,
        file: &HostPath,
    ) -> impl std::future::Future<Output = Result<Secret, DecryptError>> + Send {
        let name = file.as_str().rsplit('/').next().unwrap_or_default();
        std::future::ready(match self.files.get(name) {
            None => Err(DecryptError::Missing),
            Some(Ok(text)) => Ok(Secret::new(*text)),
            Some(Err(reason)) => Err(DecryptError::Failed(Reason::new(reason))),
        })
    }
}

#[cfg(test)]
/// What Compose could read while it ran.
#[derive(Debug, Default, Clone)]
struct Seen {
    override_text: String,
    dotenv_text: Option<String>,
    secret_files: Vec<PathBuf>,
    workspace: BTreeMap<String, String>,
}

#[cfg(test)]
/// Reads every extra file and the `.env` the runner is given, then fails or succeeds as told.
struct FakeRunner {
    failure: Option<&'static str>,
    seen: Mutex<Vec<Seen>>,
}

#[cfg(test)]
impl FakeRunner {
    fn new(failure: Option<&'static str>) -> Self {
        Self {
            failure,
            seen: Mutex::new(Vec::new()),
        }
    }
}

/// Every file of the directory that holds `file`, by name.
#[cfg(test)]
async fn snapshot(file: Option<&HostPath>) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    let Some(directory) = file.and_then(|path| std::path::Path::new(path.as_str()).parent()) else {
        return found;
    };
    let mut entries = tokio::fs::read_dir(directory).await.unwrap();
    while let Some(entry) = entries.next_entry().await.unwrap() {
        let text = tokio::fs::read_to_string(entry.path()).await.unwrap();
        found.insert(entry.file_name().to_string_lossy().into_owned(), text);
    }
    found
}

#[cfg(test)]
async fn read_all(files: &[HostPath]) -> Vec<String> {
    let mut texts = Vec::new();
    for file in files {
        texts.push(tokio::fs::read_to_string(file.as_str()).await.unwrap());
    }
    texts
}

#[cfg(test)]
impl ComposeRunner for FakeRunner {
    async fn converge(&self, request: &DeployRequest) -> Result<(), ComposeError> {
        let files = request.extras().files().as_slice();
        let dotenv: Vec<HostPath> = request.extras().dotenv().cloned().into_iter().collect();
        let seen = Seen {
            override_text: read_all(files).await.concat(),
            dotenv_text: read_all(&dotenv).await.pop(),
            secret_files: files
                .iter()
                .map(|file| PathBuf::from(file.as_str()))
                .collect(),
            workspace: snapshot(files.first().or_else(|| request.extras().dotenv())).await,
        };
        self.seen.lock().unwrap().push(seen);
        self.failure
            .map_or(Ok(()), |text| Err(ComposeError::new(Reason::new(&text))))
    }
}

#[cfg(test)]
/// The runner behind an `Arc`, so a test can read what it saw after handing it over.
struct Shared(Arc<FakeRunner>);

#[cfg(test)]
impl ComposeRunner for Shared {
    async fn converge(&self, request: &DeployRequest) -> Result<(), ComposeError> {
        self.0.converge(request).await
    }
}

#[cfg(test)]
fn root(name: &str) -> PathBuf {
    PathBuf::from("/dev/shm").join(format!("aegis-test-{}-{name}", std::process::id()))
}

#[cfg(test)]
fn service(name: &str, env_files: Vec<EnvFile>) -> Service {
    let name = ServiceName::new(name).unwrap();
    Service::new(name, ServiceSource::new(None, None)).with_env_files(EnvFiles::new(env_files))
}

#[cfg(test)]
fn env_file(path: &str, presence: Presence) -> EnvFile {
    EnvFile::new(RepoPath::new(path).unwrap(), presence)
}

#[cfg(test)]
fn request(stack: &str, services: Vec<Service>, secrets: Vec<SecretFile>) -> DeployRequest {
    let project = ProjectName::of(&StackName::new(stack).unwrap()).unwrap();
    let directory = RepoPath::new(format!("hosts/host/{stack}")).unwrap();
    let compose = ComposeSummary::new(Services::new(services), Volumes::default())
        .with_secret_files(SecretFiles::new(secrets));
    let commit = CommitSha::new("abc1234").unwrap();
    DeployRequest::new(
        Target::new(project, directory),
        commit,
        BuiltServices::default(),
    )
    .with_extras(Extras::default().with_sources(compose))
}

#[cfg(test)]
fn secret_file(name: &str, path: &str) -> SecretFile {
    SecretFile::new(SecretName::new(name).unwrap(), RepoPath::new(path).unwrap())
}

#[cfg(test)]
/// What the decorated runner is given and what its inner runner saw.
struct Harness {
    compose: DecryptingCompose<Shared, FakeDecryptor, TmpfsStore>,
    runner: Arc<FakeRunner>,
}

#[cfg(test)]
impl Harness {
    fn seen(&self) -> Vec<Seen> {
        self.runner.seen.lock().unwrap().clone()
    }

    async fn converge(&self, request: &DeployRequest) -> Result<(), ComposeError> {
        self.compose.converge(request).await
    }
}

#[cfg(test)]
fn decorated(
    store: &TmpfsStore,
    files: &[(&'static str, Result<&'static str, &'static str>)],
    failure: Option<&'static str>,
) -> Harness {
    let ports = SecretPorts::new(fake_decryptor(files), store.clone());
    harness("/srv/checkout", ports, failure)
}

#[cfg(test)]
fn decorated_in(
    checkout: impl Into<PathBuf>,
    store: &TmpfsStore,
    files: &[(&'static str, Result<&'static str, &'static str>)],
) -> Harness {
    let ports = SecretPorts::new(fake_decryptor(files), store.clone());
    harness(checkout, ports, None)
}

#[cfg(test)]
fn fake_decryptor(files: &[(&'static str, Result<&'static str, &'static str>)]) -> FakeDecryptor {
    FakeDecryptor {
        files: files.iter().copied().collect(),
    }
}

#[cfg(test)]
fn harness(
    checkout: impl Into<PathBuf>,
    ports: SecretPorts<FakeDecryptor, TmpfsStore>,
    failure: Option<&'static str>,
) -> Harness {
    let runner = Arc::new(FakeRunner::new(failure));
    let compose = DecryptingCompose::new(Shared(runner.clone()), ports, checkout);
    Harness { compose, runner }
}

#[cfg(test)]
fn app(env_files: Vec<EnvFile>) -> Service {
    service("app", env_files)
}

#[cfg(test)]
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[cfg(test)]
fn with_env(path: &str) -> DeployRequest {
    with_env_in("web", path)
}

#[cfg(test)]
fn with_env_in(stack: &str, path: &str) -> DeployRequest {
    let services = vec![app(vec![env_file(path, Presence::Required)])];
    request(stack, services, vec![])
}

#[cfg(test)]
async fn clean_up(root: &PathBuf) {
    tokio::fs::remove_dir_all(root).await.ok();
}

#[cfg(test)]
fn assert_placed(placed: &BTreeMap<String, String>) {
    assert_eq!(placed["env-app-0"], "DB_PASSWORD=hunter2\n");
    assert_eq!(placed["secret-token"], "s3cr3t-token");
    assert_eq!(placed["dotenv"], "TAG=1.2\n");
}

#[cfg(test)]
fn assert_override(text: &str) {
    assert!(text.contains("app:\n    env_file: !override\n"), "{text}");
    assert!(text.contains("aegis-web/env-app-0\""), "{text}");
    assert!(!text.contains("absent"), "{text}");
    assert!(text.contains("secrets:\n  token:\n    file: \""), "{text}");
}

#[test]
fn a_stack_is_pointed_at_decrypted_copies_that_only_exist_while_compose_runs() {
    runtime().block_on(async {
        let root = root("override");
        let files = [
            ("db.env", Ok("DB_PASSWORD=hunter2\n")),
            ("token.txt", Ok("s3cr3t-token")),
            (".env", Ok("TAG=1.2\n")),
        ];
        let compose = decorated(&TmpfsStore::at(&root), &files, None);
        let envs = vec![
            env_file("db.env", Presence::Required),
            env_file("absent.env", Presence::Optional),
        ];
        let secrets = vec![secret_file("token", "token.txt")];
        let request = request("web", vec![app(envs)], secrets);
        compose.converge(&request).await.unwrap();
        let seen = compose.seen();
        assert_override(&seen[0].override_text);
        assert_eq!(seen[0].dotenv_text.as_deref(), Some("TAG=1.2\n"));
        assert_placed(&seen[0].workspace);
        assert!(!root.join("aegis-web").exists());
        clean_up(&root).await;
    });
}

#[test]
fn every_placed_file_lives_under_the_store_root() {
    runtime().block_on(async {
        let root = root("private");
        let compose = decorated(&TmpfsStore::at(&root), &[("db.env", Ok("A=1\n"))], None);
        compose.converge(&with_env("db.env")).await.unwrap();
        clean_up(&root).await;
        let seen = compose.seen();
        assert!(
            seen[0]
                .secret_files
                .iter()
                .all(|path| path.starts_with(&root))
        );
    });
}

#[test]
fn a_file_not_encrypted_to_this_hosts_key_fails_that_stack_and_the_next_one_still_runs() {
    runtime().block_on(async {
        let root = root("denied");
        let files = [
            ("db.env", Ok("DB=1\n")),
            ("denied.env", Err("no matching key")),
            ("other.env", Ok("OTHER=2\n")),
        ];
        let compose = decorated(&TmpfsStore::at(&root), &files, None);
        let both = vec![app(vec![
            env_file("db.env", Presence::Required),
            env_file("denied.env", Presence::Required),
        ])];
        let first = compose.converge(&request("web", both, vec![])).await;
        let second = compose.converge(&with_env_in("other", "other.env")).await;
        let message = first.unwrap_err().to_string();
        assert!(message.contains("denied.env"), "{message}");
        assert!(message.contains("no matching key"), "{message}");
        assert!(second.is_ok());
        let seen = compose.seen();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].workspace["env-app-0"], "OTHER=2\n");
        assert!(!root.join("aegis-web").exists());
        clean_up(&root).await;
    });
}

#[test]
fn a_missing_required_file_fails_the_stack() {
    runtime().block_on(async {
        let root = root("missing");
        let compose = decorated(&TmpfsStore::at(&root), &[], None);
        let outcome = compose.converge(&with_env("db.env")).await;
        assert!(outcome.unwrap_err().to_string().contains("missing"));
        assert!(compose.seen().is_empty());
        clean_up(&root).await;
    });
}

#[cfg(test)]
async fn rejected_dotenv(line: &'static str) -> String {
    let root = root("reserved");
    let files = [("db.env", Ok("DB=1\n")), (".env", Ok(line))];
    let compose = decorated(&TmpfsStore::at(&root), &files, None);
    let outcome = compose.converge(&with_env("db.env")).await;
    assert!(compose.seen().is_empty());
    assert!(
        !root.join("aegis-web").exists(),
        "an earlier file was left behind"
    );
    clean_up(&root).await;
    outcome.unwrap_err().to_string()
}

#[cfg(test)]
async fn assert_rejects(dotenv: &'static str, name: &str) {
    let message = rejected_dotenv(dotenv).await;
    assert!(message.contains(&format!(".env sets {name}")), "{message}");
    assert!(!message.contains("evil"), "{message}");
}

#[test]
fn a_dotenv_that_sets_a_variable_compose_reads_rejects_the_stack_without_the_value() {
    runtime().block_on(async {
        assert_rejects("COMPOSE_FILE=evil.yaml", "COMPOSE_FILE").await;
        assert_rejects("export DOCKER_HOST=evil", "DOCKER_HOST").await;
        assert_rejects("export\tCOMPOSE_PROFILES=evil", "COMPOSE_PROFILES").await;
        assert_rejects("COMPOSE_PROJECT_NAME: evil", "COMPOSE_PROJECT_NAME").await;
        assert_rejects("\u{feff}DOCKER_CONTEXT=evil", "DOCKER_CONTEXT").await;
        assert_rejects("OK=1\nPATH=/evil", "PATH").await;
        assert_rejects("COMPOSE_PATH_SEPARATOR", "COMPOSE_PATH_SEPARATOR").await;
    });
}

#[test]
fn a_dotenv_line_that_is_not_a_plain_name_is_refused_without_echoing_it() {
    runtime().block_on(async {
        let message = rejected_dotenv("COMPOSE_ s3cr3t value=1").await;
        assert!(message.contains("plain variable name"), "{message}");
        assert!(!message.contains("s3cr3t"), "{message}");
    });
}

#[test]
fn ordinary_variables_in_a_dotenv_are_accepted() {
    runtime().block_on(async {
        let root = root("plain-dotenv");
        let files = [(
            ".env",
            Ok("# note\n\nTAG=1.2\nexport REGION=eu\nA_B.c-d: x\n"),
        )];
        let compose = decorated(&TmpfsStore::at(&root), &files, None);
        let outcome = compose.converge(&request("web", vec![], vec![])).await;
        assert!(outcome.is_ok());
        clean_up(&root).await;
    });
}

#[test]
fn an_env_file_may_set_variables_that_only_reach_the_container() {
    runtime().block_on(async {
        let root = root("container-env");
        let files = [("db.env", Ok("DOCKER_HOST=tcp://ci:2375\nPATH=/app\n"))];
        let compose = decorated(&TmpfsStore::at(&root), &files, None);
        assert!(compose.converge(&with_env("db.env")).await.is_ok());
        clean_up(&root).await;
    });
}

#[test]
fn a_compose_failure_is_reported_without_the_decrypted_values_and_still_cleans_up() {
    runtime().block_on(async {
        let root = root("redacted");
        let files = [("db.env", Ok("DB_PASSWORD=hunter2\n"))];
        let failure = Some("bad line DB_PASSWORD=hunter2");
        let compose = decorated(&TmpfsStore::at(&root), &files, failure);
        let message = compose
            .converge(&with_env("db.env"))
            .await
            .unwrap_err()
            .to_string();
        assert!(!message.contains("hunter2"), "{message}");
        assert!(!root.join("aegis-web").exists());
        clean_up(&root).await;
    });
}

#[test]
fn a_stack_without_secret_files_creates_no_workspace() {
    runtime().block_on(async {
        let root = root("none");
        let compose = decorated(&TmpfsStore::at(&root), &[], None);
        let outcome = compose.converge(&request("web", vec![], vec![])).await;
        assert!(outcome.is_ok());
        assert!(!root.exists());
    });
}

#[test]
fn a_link_that_leaves_the_stack_directory_is_refused() {
    runtime().block_on(async {
        let root = root("link");
        let checkout = root.join("checkout");
        let other = checkout.join("hosts/host/other");
        let stack = checkout.join("hosts/host/web");
        tokio::fs::create_dir_all(&other).await.unwrap();
        tokio::fs::create_dir_all(&stack).await.unwrap();
        tokio::fs::write(other.join("db.env"), "sealed")
            .await
            .unwrap();
        tokio::fs::symlink("../other", stack.join("sub"))
            .await
            .unwrap();
        let files = [("db.env", Ok("STOLEN=1\n"))];
        let store = TmpfsStore::at(root.join("store"));
        let compose = decorated_in(&checkout, &store, &files);
        let outcome = compose.converge(&with_env("sub/db.env")).await;
        let message = outcome.unwrap_err().to_string();
        assert!(message.contains("inside the stack directory"), "{message}");
        assert!(compose.seen().is_empty());
        clean_up(&root).await;
    });
}

/// A checkout with a stack `web` and a sibling stack `other`, under a scratch root.
#[cfg(test)]
async fn checkout_with_stacks(root: &std::path::Path) -> (PathBuf, PathBuf) {
    let checkout = root.join("checkout");
    let stack = checkout.join("hosts/host/web");
    tokio::fs::create_dir_all(&stack).await.unwrap();
    tokio::fs::create_dir_all(checkout.join("hosts/host/other"))
        .await
        .unwrap();
    (checkout, stack)
}

#[test]
fn a_link_that_stays_inside_the_stack_is_followed_and_the_real_path_is_decrypted() {
    runtime().block_on(async {
        let root = root("inside");
        let (checkout, stack) = checkout_with_stacks(&root).await;
        tokio::fs::create_dir_all(stack.join("real")).await.unwrap();
        tokio::fs::write(stack.join("real/db.env"), "sealed")
            .await
            .unwrap();
        tokio::fs::symlink("real", stack.join("sub")).await.unwrap();
        let files = [("db.env", Ok("A=1\n"))];
        let compose = decorated_in(&checkout, &TmpfsStore::at(root.join("store")), &files);
        compose.converge(&with_env("sub/db.env")).await.unwrap();
        assert_eq!(compose.seen()[0].workspace["env-app-0"], "A=1\n");
        clean_up(&root).await;
    });
}

#[test]
fn a_stack_directory_that_is_itself_a_link_is_confined_to_its_real_path() {
    runtime().block_on(async {
        let root = root("stacklink");
        let (checkout, stack) = checkout_with_stacks(&root).await;
        let moved = checkout.join("hosts/host/moved");
        tokio::fs::rename(&stack, &moved).await.unwrap();
        tokio::fs::symlink("moved", &stack).await.unwrap();
        tokio::fs::write(moved.join("db.env"), "sealed")
            .await
            .unwrap();
        let files = [("db.env", Ok("A=1\n"))];
        let compose = decorated_in(&checkout, &TmpfsStore::at(root.join("store")), &files);
        compose.converge(&with_env("db.env")).await.unwrap();
        assert_eq!(compose.seen()[0].workspace["env-app-0"], "A=1\n");
        clean_up(&root).await;
    });
}

#[test]
fn something_that_is_not_a_regular_file_is_refused() {
    runtime().block_on(async {
        let root = root("directory");
        let (checkout, stack) = checkout_with_stacks(&root).await;
        tokio::fs::create_dir_all(stack.join("db.env"))
            .await
            .unwrap();
        let files = [("db.env", Ok("A=1\n"))];
        let compose = decorated_in(&checkout, &TmpfsStore::at(root.join("store")), &files);
        let message = compose
            .converge(&with_env("db.env"))
            .await
            .unwrap_err()
            .to_string();
        assert!(message.contains("regular file"), "{message}");
        assert!(compose.seen().is_empty());
        clean_up(&root).await;
    });
}

#[test]
fn a_path_that_cannot_be_resolved_is_refused_not_passed_on() {
    runtime().block_on(async {
        let root = root("loop");
        let (checkout, stack) = checkout_with_stacks(&root).await;
        tokio::fs::symlink("b.env", stack.join("a.env"))
            .await
            .unwrap();
        tokio::fs::symlink("a.env", stack.join("b.env"))
            .await
            .unwrap();
        let files = [("a.env", Ok("A=1\n"))];
        let compose = decorated_in(&checkout, &TmpfsStore::at(root.join("store")), &files);
        let message = compose
            .converge(&with_env("a.env"))
            .await
            .unwrap_err()
            .to_string();
        assert!(message.contains("cannot resolve"), "{message}");
        assert!(compose.seen().is_empty());
        clean_up(&root).await;
    });
}
