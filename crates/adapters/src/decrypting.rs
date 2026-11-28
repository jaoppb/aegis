//! A [`ComposeRunner`] that decrypts a stack's secrets first.
//!
//! Every `env_file`, top-level secret file and the `.env` of the stack is decrypted onto the
//! volatile [`SecretStore`], a generated compose override points the stack at those copies, and
//! the workspace is removed once Compose returns, whether it succeeded or not. A stack that
//! cannot be decrypted fails alone, and its error never holds plaintext or `sops` output.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use aegis_application::{
    ComposeError, ComposeRunner, DecryptError, DeployRequest, ExtraFiles, Extras, Reason,
    SecretDecryptor, SecretStore, Slot,
};
use aegis_domain::{
    ComposeSummary, HostPath, Presence, ProjectName, RepoPath, Secret, SecretFile, Service,
    SlotName,
};

use crate::cli::COMPOSE_INHERITED;

/// The file type bits of a mode, and the one of a regular file.
const KIND_MASK: u32 = 0o17_0000;
const REGULAR_FILE: u32 = 0o10_0000;

/// The decryptor and the store a deploy works through.
#[derive(Debug)]
pub struct SecretPorts<Decryptor, Store> {
    decryptor: Decryptor,
    store: Store,
}

impl<Decryptor, Store> SecretPorts<Decryptor, Store> {
    /// Decrypt with `decryptor` and keep the plaintext in `store`.
    #[must_use]
    pub const fn new(decryptor: Decryptor, store: Store) -> Self {
        Self { decryptor, store }
    }
}

/// Runs `runner` after decrypting the secrets of the stack it deploys.
#[derive(Debug)]
pub struct DecryptingCompose<Runner, Decryptor, Store> {
    runner: Runner,
    ports: SecretPorts<Decryptor, Store>,
    checkout: PathBuf,
}

impl<Runner, Decryptor, Store> DecryptingCompose<Runner, Decryptor, Store> {
    /// Decrypt the stacks of the checkout at `checkout` through `ports`, then run `runner`.
    #[must_use]
    pub fn new<Checkout: Into<PathBuf>>(
        runner: Runner,
        ports: SecretPorts<Decryptor, Store>,
        checkout: Checkout,
    ) -> Self {
        let checkout = checkout.into();
        let checkout = std::path::absolute(&checkout).unwrap_or(checkout);
        Self {
            runner,
            ports,
            checkout,
        }
    }
}

/// The stack being deployed: its project and its directory in the checkout.
struct StackDirectory {
    project: ProjectName,
    path: PathBuf,
    real: PathBuf,
}

impl StackDirectory {
    fn source_of(&self, file: &RepoPath) -> Result<HostPath, Reason> {
        let full = self.path.join(file.as_str());
        HostPath::new(full.to_string_lossy()).map_err(|error| Reason::new(&error))
    }
}

/// What the placed files add up to: the plaintext to redact and the override entries.
#[derive(Default)]
struct Placed {
    plaintexts: Vec<Secret>,
    env_blocks: Vec<String>,
    secret_blocks: Vec<String>,
}

struct Session<'run, Decryptor, Store> {
    ports: &'run SecretPorts<Decryptor, Store>,
    stack: StackDirectory,
    placed: Placed,
}

impl<Decryptor: SecretDecryptor + Sync, Store: SecretStore + Sync> Session<'_, Decryptor, Store> {
    /// The plaintext of `file`, or `None` for an optional file that does not exist.
    async fn decrypt(&self, file: &RepoPath, presence: Presence) -> Result<Option<Secret>, Reason> {
        let source = self.stack.source_of(file)?;
        let source = confine(&self.stack.real, source)
            .await
            .map_err(|reason| Reason::new(&format!("{file}: {reason}")))?;
        match self.ports.decryptor.decrypt(&source).await {
            Ok(secret) => Ok(Some(secret)),
            Err(DecryptError::Missing) if presence == Presence::Optional => Ok(None),
            Err(error) => Err(Reason::new(&format!("{file}: {error}"))),
        }
    }

    /// Put `secret` in the file `name` of the workspace.
    async fn keep(&mut self, name: &str, secret: Secret) -> Result<HostPath, Reason> {
        let name = SlotName::new(name).map_err(|error| Reason::new(&error))?;
        let slot = Slot::new(self.stack.project.clone(), name);
        let path = self.ports.store.place(&slot, &secret).await;
        self.placed.plaintexts.push(secret);
        path.map_err(|error| Reason::new(&error))
    }

    /// Keep `secret` as the file `name` when there is one.
    async fn keep_if(
        &mut self,
        name: &str,
        secret: Option<Secret>,
    ) -> Result<Option<HostPath>, Reason> {
        match secret {
            Some(secret) => self.keep(name, secret).await.map(Some),
            None => Ok(None),
        }
    }

    /// The decrypted `.env`, which Compose reads for interpolation and for its own settings.
    async fn place_dotenv(&mut self) -> Result<Option<HostPath>, Reason> {
        let dotenv = RepoPath::new(".env").map_err(|error| Reason::new(&error))?;
        let secret = self.decrypt(&dotenv, Presence::Optional).await?;
        secret.as_ref().map_or(Ok(()), reject_reserved)?;
        self.keep_if("dotenv", secret).await
    }

    async fn place_all(&mut self, extras: &Extras) -> Result<Extras, Reason> {
        let compose = extras.sources();
        place_env_files(self, compose).await?;
        place_secret_files(self, compose).await?;
        let dotenv = self.place_dotenv().await?;
        let text = document(&self.placed).map(Secret::new);
        let mut files = extras.files().as_slice().to_vec();
        files.extend(self.keep_if("override.yaml", text).await?);
        let extended = extras.clone().with_files(ExtraFiles::new(files));
        Ok(match dotenv {
            Some(path) => extended.with_dotenv(path),
            None => extended,
        })
    }

    fn redacted(&self, error: &ComposeError) -> ComposeError {
        let text = error.reason().to_string();
        let clean = self
            .placed
            .plaintexts
            .iter()
            .fold(text, |text, secret| secret.redact(&text));
        ComposeError::new(Reason::new(&clean))
    }
}

/// `source` resolved through every link, which must stay inside the stack directory `stack` and be
/// a regular file: a committed link such as `sub -> ../other-stack` must not make the agent
/// decrypt another stack's file, or any file of the host, into this stack. A file that does not
/// exist is left for the decryptor to report.
async fn confine(stack: &Path, source: HostPath) -> Result<HostPath, Reason> {
    let resolved = match tokio::fs::canonicalize(source.as_str()).await {
        Ok(resolved) => resolved,
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return Ok(source),
        Err(_) => return Err(Reason::new(&"cannot resolve the file")),
    };
    let regular = tokio::fs::metadata(&resolved).await;
    if !resolved.starts_with(stack)
        || !regular.is_ok_and(|metadata| metadata.mode() & KIND_MASK == REGULAR_FILE)
    {
        return Err(Reason::new(
            &"not a regular file inside the stack directory",
        ));
    }
    HostPath::new(resolved.to_string_lossy()).map_err(|error| Reason::new(&error))
}

/// A YAML double-quoted scalar for `path`.
fn quoted(path: &HostPath) -> String {
    let escaped = path.as_str().replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// The override entry that replaces the `env_file` list of `service` by the decrypted copies, or
/// nothing for a service without env files.
fn env_block(service: &Service, paths: &[HostPath]) -> Option<String> {
    let header = format!("  {}:\n    env_file: !override", service.name());
    let entries = paths
        .iter()
        .map(|path| format!("      - {}\n", quoted(path)));
    let listed = format!("{header}\n{}", entries.collect::<String>());
    let empty = format!("{header} []\n");
    let block = if paths.is_empty() { empty } else { listed };
    (!service.env_files().is_empty()).then_some(block)
}

/// The override entry that points the secret `file` at its decrypted copy.
fn secret_block(file: &SecretFile, path: &HostPath) -> String {
    format!("  {}:\n    file: {}\n", file.name(), quoted(path))
}

async fn place_service_env<Decryptor, Store>(
    session: &mut Session<'_, Decryptor, Store>,
    service: &Service,
) -> Result<Vec<HostPath>, Reason>
where
    Decryptor: SecretDecryptor + Sync,
    Store: SecretStore + Sync,
{
    let mut paths = Vec::new();
    for (index, env) in service.env_files().iter().enumerate() {
        let secret = session.decrypt(env.path(), env.presence()).await?;
        let name = format!("env-{}-{index}", service.name());
        paths.extend(session.keep_if(&name, secret).await?);
    }
    Ok(paths)
}

async fn place_env_files<Decryptor, Store>(
    session: &mut Session<'_, Decryptor, Store>,
    compose: &ComposeSummary,
) -> Result<(), Reason>
where
    Decryptor: SecretDecryptor + Sync,
    Store: SecretStore + Sync,
{
    for service in compose.services() {
        let paths = place_service_env(session, service).await?;
        session.placed.env_blocks.extend(env_block(service, &paths));
    }
    Ok(())
}

async fn place_secret_files<Decryptor, Store>(
    session: &mut Session<'_, Decryptor, Store>,
    compose: &ComposeSummary,
) -> Result<(), Reason>
where
    Decryptor: SecretDecryptor + Sync,
    Store: SecretStore + Sync,
{
    for file in compose.secret_files() {
        let secret = session.decrypt(file.path(), Presence::Required).await?;
        let name = format!("secret-{}", file.name());
        let path = session.keep_if(&name, secret).await?;
        let block = path.map(|path| secret_block(file, &path));
        session.placed.secret_blocks.extend(block);
    }
    Ok(())
}

/// The compose override that re-points env files and secret files, if there is any to re-point.
fn document(placed: &Placed) -> Option<String> {
    let services = (!placed.env_blocks.is_empty())
        .then(|| format!("services:\n{}", placed.env_blocks.concat()));
    let secrets = (!placed.secret_blocks.is_empty())
        .then(|| format!("secrets:\n{}", placed.secret_blocks.concat()));
    let parts: Vec<String> = services.into_iter().chain(secrets).collect();
    (!parts.is_empty()).then(|| parts.concat())
}

/// The variable a `.env` line sets, read as Compose does: a BOM is dropped, `export` and a colon
/// instead of `=` are accepted, and a line with no value still names its variable. Blank lines and
/// comments name none.
fn variable_name(line: &str) -> Option<&str> {
    let line = line.trim_start_matches('\u{feff}').trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let exported = line.strip_prefix("export");
    let line = exported
        .filter(|rest| rest.starts_with(char::is_whitespace))
        .map_or(line, str::trim_start);
    line.split(['=', ':']).next().map(str::trim)
}

/// Names that steer Compose or the programs it runs: a stack's secrets must not set them.
fn is_reserved(name: &str) -> bool {
    name.starts_with("COMPOSE_") || name.starts_with("DOCKER_") || COMPOSE_INHERITED.contains(&name)
}

fn is_identifier(name: &str) -> bool {
    let word = |character: char| character.is_ascii_alphanumeric() || "_.-".contains(character);
    !name.is_empty() && name.chars().all(word)
}

/// A line that is not a plain name is refused without echoing it: it is plaintext.
fn check_variable(name: &str) -> Result<(), Reason> {
    if !is_identifier(name) {
        return Err(Reason::new(
            &".env has a line that does not start with a plain variable name",
        ));
    }
    if is_reserved(name) {
        return Err(Reason::new(&format!(
            ".env sets {name}, which Compose or the agent reads"
        )));
    }
    Ok(())
}

fn reject_reserved(dotenv: &Secret) -> Result<(), Reason> {
    let mut names = dotenv.expose().lines().filter_map(variable_name);
    names.try_for_each(check_variable)
}

impl<Runner, Decryptor, Store> DecryptingCompose<Runner, Decryptor, Store>
where
    Runner: ComposeRunner + Sync,
    Decryptor: SecretDecryptor + Sync,
    Store: SecretStore + Sync,
{
    async fn deploy(&self, request: &DeployRequest) -> Result<(), ComposeError> {
        let path = self.checkout.join(request.target().directory().as_str());
        let real = tokio::fs::canonicalize(&path).await;
        let mut session = Session {
            ports: &self.ports,
            stack: StackDirectory {
                project: request.target().project().clone(),
                real: real.unwrap_or_else(|_| path.clone()),
                path,
            },
            placed: Placed::default(),
        };
        let extras = session
            .place_all(request.extras())
            .await
            .map_err(ComposeError::new)?;
        let prepared = request.clone().with_extras(extras);
        let done = self.runner.converge(&prepared).await;
        done.map_err(|error| session.redacted(&error))
    }
}

impl<Runner, Decryptor, Store> ComposeRunner for DecryptingCompose<Runner, Decryptor, Store>
where
    Runner: ComposeRunner + Sync,
    Decryptor: SecretDecryptor + Sync,
    Store: SecretStore + Sync,
{
    async fn converge(&self, request: &DeployRequest) -> Result<(), ComposeError> {
        let outcome = self.deploy(request).await;
        let removed = self.ports.store.discard(request.target().project()).await;
        outcome?;
        removed.map_err(|error| ComposeError::new(Reason::new(&error)))
    }
}
