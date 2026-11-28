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
    let decryptor = FakeDecryptor {
        files: files.iter().copied().collect(),
    };
    let runner = Arc::new(FakeRunner::new(failure));
    let ports = SecretPorts::new(decryptor, store.clone());
    let compose = DecryptingCompose::new(Shared(runner.clone()), ports, "/srv/checkout");
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
    let services = vec![app(vec![env_file(path, Presence::Required)])];
    request("web", services, vec![])
}

#[cfg(test)]
async fn clean_up(root: &PathBuf) {
    tokio::fs::remove_dir_all(root).await.ok();
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
        let text = &seen[0].override_text;
        assert!(text.contains("app:\n    env_file: !override\n"), "{text}");
        assert!(text.contains("aegis-web/env-app-0\""), "{text}");
        assert!(!text.contains("absent"), "{text}");
        assert!(text.contains("secrets:\n  token:\n    file: \""), "{text}");
        assert_eq!(seen[0].dotenv_text.as_deref(), Some("TAG=1.2\n"));
        assert!(!root.join("aegis-web").exists());
        clean_up(&root).await;
    });
}

#[test]
fn plaintext_is_written_only_under_the_tmpfs_workspace() {
    runtime().block_on(async {
        let root = root("private");
        let compose = decorated(&TmpfsStore::at(&root), &[("db.env", Ok("A=1\n"))], None);
        compose.converge(&with_env("db.env")).await.unwrap();
        let seen = compose.seen();
        assert!(
            seen[0]
                .secret_files
                .iter()
                .all(|path| path.starts_with(&root))
        );
        assert!(root.starts_with("/dev/shm"));
        clean_up(&root).await;
    });
}

#[test]
fn a_file_not_encrypted_to_this_hosts_key_fails_that_stack_and_leaves_nothing() {
    runtime().block_on(async {
        let root = root("denied");
        let files = [("db.env", Err("no matching key"))];
        let compose = decorated(&TmpfsStore::at(&root), &files, None);
        let first = compose.converge(&with_env("db.env")).await;
        let second = compose.converge(&request("other", vec![], vec![])).await;
        let message = first.unwrap_err().to_string();
        assert!(message.contains("db.env"), "{message}");
        assert!(message.contains("no matching key"), "{message}");
        assert!(second.is_ok());
        assert_eq!(compose.seen().len(), 1);
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
    let compose = decorated(&TmpfsStore::at(&root), &[(".env", Ok(line))], None);
    let outcome = compose.converge(&request("web", vec![], vec![])).await;
    assert!(compose.seen().is_empty());
    clean_up(&root).await;
    outcome.unwrap_err().to_string()
}

#[test]
fn a_dotenv_that_sets_a_variable_compose_reads_rejects_the_stack_without_the_value() {
    runtime().block_on(async {
        let compose_file = rejected_dotenv("COMPOSE_FILE=evil.yaml").await;
        let docker_host = rejected_dotenv("export DOCKER_HOST=tcp://x").await;
        let path = rejected_dotenv("PATH=/evil").await;
        assert!(
            compose_file.contains(".env sets COMPOSE_FILE"),
            "{compose_file}"
        );
        assert!(
            docker_host.contains(".env sets DOCKER_HOST"),
            "{docker_host}"
        );
        assert!(path.contains(".env sets PATH"), "{path}");
        assert!(!compose_file.contains("evil") && !docker_host.contains("tcp"));
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

#[cfg(test)]
/// Whether the file system that holds this crate is tmpfs, which a test of the refusal needs not.
async fn checkout_is_on_tmpfs() -> bool {
    let mounts = tokio::fs::read_to_string("/proc/self/mountinfo").await;
    let here = tokio::fs::canonicalize(env!("CARGO_MANIFEST_DIR")).await;
    let (mounts, here) = (mounts.unwrap(), here.unwrap());
    mounts.lines().any(|line| {
        let point = line.split(' ').nth(4).unwrap_or("/nowhere");
        line.contains(" - tmpfs ") && here.starts_with(point) && point != "/"
    })
}

#[cfg(test)]
async fn refused(disk: &PathBuf) -> bool {
    let compose = decorated(&TmpfsStore::at(disk), &[("db.env", Ok("A=1\n"))], None);
    let outcome = compose.converge(&with_env("db.env")).await;
    let message = outcome.unwrap_err().to_string();
    message.contains("not on tmpfs") && compose.seen().is_empty()
}

#[test]
fn the_store_refuses_a_directory_that_is_not_tmpfs() {
    runtime().block_on(async {
        let disk = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/aegis-not-tmpfs");
        assert!(checkout_is_on_tmpfs().await || refused(&disk).await);
        clean_up(&disk).await;
    });
}
