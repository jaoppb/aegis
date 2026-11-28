//! `aegis bootstrap`: the one-time installation of the agent and the controller on a host.
//!
//! This module holds the values the command takes, the ports it needs and the use case that
//! drives them; it does no IO.

use std::fmt;
use std::future::Future;

use aegis_domain::{
    AegisRole, HostName, HostPath, ProjectName, RepoPath, Secret, StackName, TailnetTag,
    TailnetTags, ValueError,
};

use crate::enroll::StateProbe;
use crate::render::{self, Content, PlannedFile, RenderedStack, Stacks};
use crate::source::Reason;
use crate::tailnet::{AuthKey, Enrolled, Enrollment, KeyMinter, TailnetError};

/// Define a validated text value: private field, validating constructor, `as_str`.
macro_rules! text_value {
    ($(#[$meta:meta])* $name:ident, $validate:path) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq)]
        pub struct $name(String);

        impl $name {
            /// Validate `value` and wrap it.
            ///
            /// # Errors
            ///
            /// Returns a [`ValueError`] when `value` is not valid for this type.
            pub fn new(value: impl Into<String>) -> Result<Self, ValueError> {
                let value = value.into();
                $validate(&value)?;
                Ok(Self(value))
            }

            /// The validated text.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

const fn is_age_char(candidate: char) -> bool {
    candidate.is_ascii_lowercase() || candidate.is_ascii_digit()
}

fn validate_recipient(value: &str) -> Result<(), ValueError> {
    let body = value.strip_prefix("age1").ok_or(ValueError::BadPrefix)?;
    if body.is_empty() {
        return Err(ValueError::Empty);
    }
    let forbidden = body.chars().find(|candidate| !is_age_char(*candidate));
    forbidden.map_or(Ok(()), |found| Err(ValueError::ForbiddenChar(found)))
}

const fn is_tag_char(candidate: char) -> bool {
    candidate.is_ascii_alphanumeric() || matches!(candidate, '_' | '.' | '-')
}

fn validate_image_tag(value: &str) -> Result<(), ValueError> {
    if value.is_empty() {
        return Err(ValueError::Empty);
    }
    let forbidden = value.chars().find(|candidate| !is_tag_char(*candidate));
    if let Some(found) = forbidden {
        return Err(ValueError::ForbiddenChar(found));
    }
    let first = value.chars().next().filter(|first| is_tag_char(*first));
    first
        .filter(|first| !matches!(first, '.' | '-'))
        .map_or(Err(ValueError::ForbiddenChar('-')), |_| Ok(()))
}

fn validate_repo_url(value: &str) -> Result<(), ValueError> {
    if value.is_empty() {
        return Err(ValueError::Empty);
    }
    if value.starts_with('-') {
        return Err(ValueError::ForbiddenChar('-'));
    }
    let forbidden = value
        .chars()
        .find(|candidate| candidate.is_whitespace() || candidate.is_control());
    if let Some(found) = forbidden {
        return Err(ValueError::ForbiddenChar(found));
    }
    reject_password(value)
}

/// The schemes whose userinfo is a login name, not a secret: `ssh://git@host`.
const SSH_SCHEMES: [&str; 3] = ["ssh", "git+ssh", "ssh+git"];

/// Refuse credentials in a URL: a token or `user:password` before the `@` would be visible in the
/// process list and written to the checkout's git config. Only the user of an SSH URL is
/// allowed, scp-style remotes (`git@host:path`) have no scheme and carry no secret, and the
/// `helper::url` transport syntax is refused outright because it hides the real scheme.
fn reject_password(value: &str) -> Result<(), ValueError> {
    if value.contains("::") {
        return Err(ValueError::EmbeddedPassword);
    }
    let Some((scheme, rest)) = value.split_once("://") else {
        return Ok(());
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let secret = authority.rsplit_once('@').is_some_and(|(user, _)| {
        let ssh = SSH_SCHEMES
            .iter()
            .any(|known| known.eq_ignore_ascii_case(scheme));
        user.contains(':') || !ssh
    });
    if secret {
        return Err(ValueError::EmbeddedPassword);
    }
    Ok(())
}

fn validate_controller_url(value: &str) -> Result<(), ValueError> {
    if !(value.starts_with("http://") || value.starts_with("https://")) {
        return Err(ValueError::BadScheme);
    }
    let forbidden = value.chars().find(|candidate| {
        candidate.is_whitespace()
            || candidate.is_control()
            || matches!(candidate, '"' | '\\' | '$' | '\'')
    });
    forbidden.map_or(Ok(()), |found| Err(ValueError::ForbiddenChar(found)))
}

text_value!(
    /// The public half of the host's age key, which goes into `.sops.yaml`.
    AgeRecipient,
    validate_recipient
);
text_value!(
    /// The tag of the Aegis images to run, such as `0.1.0`.
    ImageTag,
    validate_image_tag
);
text_value!(
    /// The git remote of the repository. It never carries a password: use an SSH key or a git
    /// credential helper.
    RepoUrl,
    validate_repo_url
);
text_value!(
    /// The URL the agent reaches the controller at.
    ControllerUrl,
    validate_controller_url
);

impl fmt::Debug for RepoUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RepoUrl(<redacted>)")
    }
}

impl fmt::Debug for AgeRecipient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "AgeRecipient({})", self.0)
    }
}

impl fmt::Debug for ImageTag {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ImageTag({})", self.0)
    }
}

impl fmt::Debug for ControllerUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ControllerUrl({})", self.0)
    }
}

impl fmt::Display for AgeRecipient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl fmt::Display for ImageTag {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The host's age key: where the private half lives and its public recipient.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKey {
    recipient: AgeRecipient,
    path: HostPath,
}

impl HostKey {
    /// The key stored at `path` with `recipient`.
    #[must_use]
    pub const fn new(recipient: AgeRecipient, path: HostPath) -> Self {
        Self { recipient, path }
    }

    /// The public recipient.
    #[must_use]
    pub const fn recipient(&self) -> &AgeRecipient {
        &self.recipient
    }

    /// The file holding the private key.
    #[must_use]
    pub const fn path(&self) -> &HostPath {
        &self.path
    }
}

/// Where the repository and the controller are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sources {
    repo: RepoUrl,
    controller: ControllerUrl,
    checkout: HostPath,
}

impl Sources {
    /// The `repo` cloned into `checkout`, and the `controller` the agent reports to.
    #[must_use]
    pub const fn new(repo: RepoUrl, controller: ControllerUrl, checkout: HostPath) -> Self {
        Self {
            repo,
            controller,
            checkout,
        }
    }

    /// The git remote.
    #[must_use]
    pub const fn repo(&self) -> &RepoUrl {
        &self.repo
    }

    /// The controller URL.
    #[must_use]
    pub const fn controller(&self) -> &ControllerUrl {
        &self.controller
    }

    /// The absolute path of the checkout.
    #[must_use]
    pub const fn checkout(&self) -> &HostPath {
        &self.checkout
    }
}

/// Whether the controller is installed on this host too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControllerChoice {
    /// Only the agent.
    Skip,
    /// The controller as well, with its credentials and the tags its stacks' nodes may carry.
    Install(ControllerSetup),
}

/// What the controller needs besides its image: the OAuth credentials it mints keys with and the
/// ACL tags it lets a stack's node carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerSetup {
    credentials: Secret,
    tags: TailnetTags,
}

impl ControllerSetup {
    /// A controller with the dotenv text of its OAuth `credentials` (`AEGIS_TS_CLIENT_ID` and
    /// `AEGIS_TS_CLIENT_SECRET`), which is encrypted to the host key before it is written so it is
    /// never plaintext in git or on disk, and the `tags` stack nodes may carry
    /// (`AEGIS_ENROLL_TAGS`). The running container's environment still holds the decrypted values.
    #[must_use]
    pub const fn new(credentials: Secret, tags: TailnetTags) -> Self {
        Self { credentials, tags }
    }

    /// The dotenv text of the credentials.
    #[must_use]
    pub const fn credentials(&self) -> &Secret {
        &self.credentials
    }

    /// The value of `AEGIS_ENROLL_TAGS`: the allowed tags, comma-separated.
    #[must_use]
    pub fn enroll_tags(&self) -> String {
        let names: Vec<&str> = self.tags.iter().map(TailnetTag::as_str).collect();
        names.join(",")
    }
}

/// What `aegis bootstrap` is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapPlan {
    host: HostName,
    sources: Sources,
    image: ImageTag,
    controller: ControllerChoice,
}

impl BootstrapPlan {
    /// Install `image` on `host`, with the `sources`; the agent only, until
    /// [`BootstrapPlan::with_controller`] says otherwise.
    #[must_use]
    pub const fn new(host: HostName, sources: Sources, image: ImageTag) -> Self {
        Self {
            host,
            sources,
            image,
            controller: ControllerChoice::Skip,
        }
    }

    /// The same plan with the `controller` choice.
    #[must_use]
    pub fn with_controller(self, controller: ControllerChoice) -> Self {
        Self { controller, ..self }
    }

    /// The host name.
    #[must_use]
    pub const fn host(&self) -> &HostName {
        &self.host
    }

    /// The repository, controller and checkout.
    #[must_use]
    pub const fn sources(&self) -> &Sources {
        &self.sources
    }

    /// The image tag.
    #[must_use]
    pub const fn image(&self) -> &ImageTag {
        &self.image
    }

    /// The controller choice.
    #[must_use]
    pub const fn controller(&self) -> &ControllerChoice {
        &self.controller
    }
}

/// The text of a file Bootstrap writes.
#[derive(Clone, PartialEq, Eq)]
pub struct FileText(String);

impl FileText {
    /// Wrap `text`.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// The text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for FileText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "FileText({} bytes)", self.0.len())
    }
}

/// What to do when the file is already there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Existing {
    /// Replace it with the new text.
    Replace,
    /// Leave it as it is.
    Keep,
}

/// A file of a stack, relative to the bootstrap directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedFile {
    path: RepoPath,
    text: FileText,
}

impl RenderedFile {
    /// The file at `path` holding `text`.
    #[must_use]
    pub const fn new(path: RepoPath, text: FileText) -> Self {
        Self { path, text }
    }

    /// Where the file goes.
    #[must_use]
    pub const fn path(&self) -> &RepoPath {
        &self.path
    }

    /// What it holds.
    #[must_use]
    pub const fn text(&self) -> &FileText {
        &self.text
    }
}

/// What writing a file did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Written {
    /// The file was created or its text changed.
    Changed,
    /// The file already held this text, or was kept.
    Unchanged,
}

impl Written {
    /// `Changed` when either side changed something.
    #[must_use]
    pub const fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unchanged, Self::Unchanged) => Self::Unchanged,
            _ => Self::Changed,
        }
    }
}

/// A step of the bootstrap failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootstrapError {
    /// A value derived for a stack is invalid.
    Invalid(ValueError),
    /// The age key could not be read or generated.
    AgeKey(Reason),
    /// The repository could not be cloned or updated.
    Checkout(Reason),
    /// A stack file could not be written.
    Write(Reason),
    /// The OAuth credentials could not be encrypted.
    Encrypt(Reason),
    /// The sidecar's state could not be read.
    State(Reason),
    /// Tailscale did not mint a key.
    Mint(TailnetError),
    /// Compose could not bring a stack up.
    Run(Reason),
}

impl fmt::Display for BootstrapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(error) => write!(formatter, "invalid value: {error}"),
            Self::AgeKey(reason) => write!(formatter, "age key: {reason}"),
            Self::Checkout(reason) => write!(formatter, "checkout: {reason}"),
            Self::Write(reason) => write!(formatter, "writing a stack file: {reason}"),
            Self::Encrypt(reason) => write!(formatter, "encrypting the credentials: {reason}"),
            Self::State(reason) => write!(formatter, "reading the sidecar state: {reason}"),
            Self::Mint(error) => write!(formatter, "minting a key: {error}"),
            Self::Run(reason) => write!(formatter, "starting a stack: {reason}"),
        }
    }
}

impl std::error::Error for BootstrapError {}

impl From<ValueError> for BootstrapError {
    fn from(error: ValueError) -> Self {
        Self::Invalid(error)
    }
}

/// The host's own files: the age key, the repository checkout and the stack files.
pub trait HostFiles {
    /// The host's age key, generated when there is none yet. A key that exists is never replaced.
    ///
    /// # Errors
    ///
    /// Returns [`BootstrapError::AgeKey`] when the key cannot be read or generated.
    fn ensure_age_key(&self) -> impl Future<Output = Result<HostKey, BootstrapError>> + Send;

    /// Clone the repository, or fetch it when it is already cloned. The checkout is read-only:
    /// nothing is ever pushed.
    ///
    /// # Errors
    ///
    /// Returns [`BootstrapError::Checkout`] when git fails.
    fn ensure_checkout(
        &self,
        repo: &RepoUrl,
    ) -> impl Future<Output = Result<(), BootstrapError>> + Send;

    /// Write `file` below the bootstrap directory.
    ///
    /// # Errors
    ///
    /// Returns [`BootstrapError::Write`] when the file cannot be written.
    fn write(
        &self,
        file: &RenderedFile,
        existing: Existing,
    ) -> impl Future<Output = Result<Written, BootstrapError>> + Send;
}

/// Encrypts a secret to an age recipient.
pub trait SecretEncryptor {
    /// The SOPS-encrypted dotenv text of `plaintext`, readable only with the key of `recipient`.
    ///
    /// # Errors
    ///
    /// Returns [`BootstrapError::Encrypt`] when `sops` fails. The error never holds the plaintext.
    fn encrypt(
        &self,
        plaintext: &Secret,
        recipient: &AgeRecipient,
    ) -> impl Future<Output = Result<FileText, BootstrapError>> + Send;
}

/// Gives a node the key it joins the tailnet with.
pub trait NodeKeys {
    /// A key for the node of the stack `project` in `role`, or `None` when its sidecar already
    /// has state.
    ///
    /// # Errors
    ///
    /// Returns [`BootstrapError::State`] when the state cannot be read and
    /// [`BootstrapError::Mint`] when no key can be minted.
    fn key_for(
        &self,
        project: &ProjectName,
        role: AegisRole,
    ) -> impl Future<Output = Result<Option<AuthKey>, BootstrapError>> + Send;
}

/// The nodes of Aegis itself: a key is minted for one only while its sidecar has no state.
///
/// An Aegis node carries its role tag, which a stack's own enrolment refuses, so this goes
/// around that tag check and nothing else: the minting path is the one [`Enrollment`] has, so a
/// rerun never creates a second node.
#[derive(Debug)]
pub struct AegisNodes<Probe, Minter> {
    probe: Probe,
    enrollment: Enrollment<Minter>,
}

impl<Probe, Minter> AegisNodes<Probe, Minter> {
    /// Read sidecar state with `probe` and mint through `enrollment`.
    #[must_use]
    pub const fn new(probe: Probe, enrollment: Enrollment<Minter>) -> Self {
        Self { probe, enrollment }
    }
}

impl<Probe: StateProbe + Sync, Minter: KeyMinter + Sync> NodeKeys for AegisNodes<Probe, Minter> {
    async fn key_for(
        &self,
        project: &ProjectName,
        role: AegisRole,
    ) -> Result<Option<AuthKey>, BootstrapError> {
        let state = self.probe.probe(project).await;
        let state = state.map_err(BootstrapError::State)?;
        let tags = TailnetTags::new(vec![role.tag()?]);
        let enrolled = self.enrollment.enroll_tagged(&tags, state).await;
        Ok(match enrolled.map_err(BootstrapError::Mint)? {
            Enrolled::Minted(key) => Some(key),
            Enrolled::AlreadyEnrolled => None,
        })
    }
}

/// Brings a stack of the bootstrap directory up.
pub trait StackRunner {
    /// `docker compose up -d` for the stack `stack`, with `key` as `TS_AUTHKEY` in the process
    /// environment of Compose when it is given.
    ///
    /// # Errors
    ///
    /// Returns [`BootstrapError::Run`] when Compose fails. The error never holds the key.
    fn start(
        &self,
        stack: &StackName,
        key: Option<&AuthKey>,
    ) -> impl Future<Output = Result<(), BootstrapError>> + Send;
}

/// What happened to the node of a stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Joined {
    /// A key was minted, so the node joins the tailnet now.
    NewNode,
    /// The sidecar already had state: nothing was minted.
    ExistingNode,
}

/// What bootstrap did for one stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackOutcome {
    stack: StackName,
    joined: Joined,
    files: Written,
}

impl StackOutcome {
    /// The outcome for `stack`.
    #[must_use]
    pub const fn new(stack: StackName, joined: Joined, files: Written) -> Self {
        Self {
            stack,
            joined,
            files,
        }
    }

    /// Whether the stack needed anything: a new node or changed files.
    #[must_use]
    pub fn is_noop(&self) -> bool {
        self.joined == Joined::ExistingNode && self.files == Written::Unchanged
    }
}

impl fmt::Display for StackOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = match self.joined {
            Joined::NewNode => "joined the tailnet",
            Joined::ExistingNode => "already on the tailnet",
        };
        let files = match self.files {
            Written::Changed => "files written",
            Written::Unchanged => "files unchanged",
        };
        write!(formatter, "aegis/{}: {node}, {files}", self.stack)
    }
}

/// The outcomes of every stack.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcomes(Vec<StackOutcome>);

impl Outcomes {
    /// The outcomes, in the order the stacks were brought up.
    pub fn iter(&self) -> std::slice::Iter<'_, StackOutcome> {
        self.0.iter()
    }

    fn push(&mut self, outcome: StackOutcome) {
        self.0.push(outcome);
    }
}

impl<'items> IntoIterator for &'items Outcomes {
    type Item = &'items StackOutcome;
    type IntoIter = std::slice::Iter<'items, StackOutcome>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

/// What a bootstrap did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapReport {
    recipient: AgeRecipient,
    outcomes: Outcomes,
}

impl BootstrapReport {
    /// The recipient to put in `.sops.yaml`, and what happened to each stack.
    #[must_use]
    pub const fn new(recipient: AgeRecipient, outcomes: Outcomes) -> Self {
        Self {
            recipient,
            outcomes,
        }
    }

    /// The host's age recipient.
    #[must_use]
    pub const fn recipient(&self) -> &AgeRecipient {
        &self.recipient
    }

    /// Whether this run changed nothing.
    #[must_use]
    pub fn is_noop(&self) -> bool {
        (&self.outcomes).into_iter().all(StackOutcome::is_noop)
    }

    /// The per-stack outcomes.
    #[must_use]
    pub const fn outcomes(&self) -> &Outcomes {
        &self.outcomes
    }
}

impl fmt::Display for BootstrapReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(formatter, "age recipient: {}", self.recipient)?;
        let mut lines = self.outcomes.into_iter();
        lines.try_for_each(|outcome| writeln!(formatter, "{outcome}"))?;
        let note = self.is_noop().then_some("nothing to do\n");
        formatter.write_str(note.unwrap_or_default())
    }
}

/// The ports that touch the host's files: the stack files and the encryption of the credentials.
#[derive(Debug)]
pub struct HostSide<Files, Cipher> {
    files: Files,
    cipher: Cipher,
}

impl<Files, Cipher> HostSide<Files, Cipher> {
    /// Write files through `files` and encrypt credentials with `cipher`.
    #[must_use]
    pub const fn new(files: Files, cipher: Cipher) -> Self {
        Self { files, cipher }
    }
}

/// The use case: install the agent, and the controller when asked, from the images.
#[derive(Debug)]
pub struct Bootstrap<Files, Cipher, Keys, Runner> {
    host: HostSide<Files, Cipher>,
    keys: Keys,
    runner: Runner,
}

impl<Files, Cipher, Keys, Runner> Bootstrap<Files, Cipher, Keys, Runner> {
    /// A bootstrap built on the given ports.
    #[must_use]
    pub const fn new(host: HostSide<Files, Cipher>, keys: Keys, runner: Runner) -> Self {
        Self { host, keys, runner }
    }
}

impl<Files, Cipher, Keys, Runner> Bootstrap<Files, Cipher, Keys, Runner>
where
    Files: HostFiles + Sync,
    Cipher: SecretEncryptor + Sync,
    Keys: NodeKeys + Sync,
    Runner: StackRunner + Sync,
{
    /// Install what `plan` asks for. Running it again changes nothing that already exists: the age
    /// key, the sidecar's node and the encrypted credentials are reused.
    ///
    /// # Errors
    ///
    /// Returns the [`BootstrapError`] of the first step that fails.
    pub async fn run(&self, plan: &BootstrapPlan) -> Result<BootstrapReport, BootstrapError> {
        let key = self.host.files.ensure_age_key().await?;
        self.host
            .files
            .ensure_checkout(plan.sources().repo())
            .await?;
        let stacks = render::stacks(plan, &key)?;
        let outcomes = install_all(self, stacks, &key).await?;
        Ok(BootstrapReport::new(key.recipient().clone(), outcomes))
    }

    async fn install(
        &self,
        stack: &RenderedStack,
        key: &HostKey,
    ) -> Result<StackOutcome, BootstrapError> {
        let files = write_files(&self.host, stack, key).await?;
        let node = self.keys.key_for(stack.project(), stack.role()).await?;
        let joined = node
            .as_ref()
            .map_or(Joined::ExistingNode, |_| Joined::NewNode);
        self.runner.start(stack.name(), node.as_ref()).await?;
        Ok(StackOutcome::new(stack.name().clone(), joined, files))
    }
}

async fn install_all<Files, Cipher, Keys, Runner>(
    bootstrap: &Bootstrap<Files, Cipher, Keys, Runner>,
    stacks: Stacks,
    key: &HostKey,
) -> Result<Outcomes, BootstrapError>
where
    Files: HostFiles + Sync,
    Cipher: SecretEncryptor + Sync,
    Keys: NodeKeys + Sync,
    Runner: StackRunner + Sync,
{
    let mut outcomes = Outcomes::default();
    for stack in stacks {
        outcomes.push(bootstrap.install(&stack, key).await?);
    }
    Ok(outcomes)
}

async fn write_files<Files: HostFiles + Sync, Cipher: SecretEncryptor + Sync>(
    host: &HostSide<Files, Cipher>,
    stack: &RenderedStack,
    key: &HostKey,
) -> Result<Written, BootstrapError> {
    let mut result = Written::Unchanged;
    for file in stack.files() {
        let text = text_of(host, file, key).await?;
        let rendered = RenderedFile::new(file.path().clone(), text);
        let written = host.files.write(&rendered, file.existing()).await?;
        result = result.merge(written);
    }
    Ok(result)
}

async fn text_of<Files: Sync, Cipher: SecretEncryptor + Sync>(
    host: &HostSide<Files, Cipher>,
    file: &PlannedFile,
    key: &HostKey,
) -> Result<FileText, BootstrapError> {
    match file.content() {
        Content::Plain(text) => Ok(text.clone()),
        Content::Encrypted(secret) => host.cipher.encrypt(secret, key.recipient()).await,
    }
}
