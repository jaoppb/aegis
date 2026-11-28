//! The compose files Bootstrap renders for the agent and the controller. Everything a template
//! takes is validated first, so no value can break out of its YAML scalar or its volume mapping.

use aegis_domain::{AegisRole, HostName, HostPath, ProjectName, RepoPath, StackName, ValueError};

use crate::bootstrap::{
    AgentEndpoint, AgentUrl, BootstrapPlan, ControllerChoice, ControllerSetup, Existing, FileText,
    HostKey, ImageTag,
};
use crate::sidecar::{KEY_VARIABLE, STATE_DIRECTORY, STATE_VOLUME, TAILSCALE_IMAGE};

/// The Aegis images, without their tag.
const AGENT_IMAGE: &str = "ghcr.io/jaoppb/aegis-agent";
const CONTROLLER_IMAGE: &str = "ghcr.io/jaoppb/aegis-controller";

/// The port both daemons listen on, each inside its own sidecar's network namespace.
pub const LISTEN: &str = "0.0.0.0:7480";

/// The port of [`LISTEN`], which the agents are reached on.
const PORT: &str = "7480";

/// Where the host's age key and the volatile workspace are mounted inside the agent container.
const KEY_IN_CONTAINER: &str = "/etc/aegis/age.txt";
const RUNTIME_DIRECTORY: &str = "/run/aegis-agent";

/// The controller's disposable checkout, inside its container: it starts empty on every new
/// container, which the controller accepts.
const CONTROLLER_WORKDIR: &str = "/var/lib/aegis/work";

/// The file of the controller that holds its OAuth credentials, encrypted.
const CONTROLLER_ENV: &str = "controller.env";

const SIDECAR: &str = r#"  tailscale:
    image: "@@TAILSCALE@@"
    restart: unless-stopped
    cap_add:
      - NET_ADMIN
    device_cgroup_rules:
      - "c 10:200 rwm"
    environment:
      TS_AUTHKEY: "${@@KEY@@:-}"
      TS_HOSTNAME: "@@NODE@@"
      TS_USERSPACE: "false"
      TS_ACCEPT_DNS: "true"
      TS_STATE_DIR: "@@STATE_DIRECTORY@@"
      TS_SOCKET: /var/run/tailscale/tailscaled.sock
    healthcheck:
      test:
        - CMD-SHELL
        - "tailscale --socket=/var/run/tailscale/tailscaled.sock status --json | grep -q '\"BackendState\": \"Running\"'"
      interval: 5s
      timeout: 5s
      retries: 24
    volumes:
      - "@@STATE_VOLUME@@:@@STATE_DIRECTORY@@"
      - "/dev/net/tun:/dev/net/tun"
      - "socket:/var/run/tailscale"
"#;

const AGENT: &str = r#"  agent:
    image: "@@IMAGE@@"
    restart: unless-stopped
    network_mode: "service:tailscale"
    depends_on:
      tailscale:
        condition: service_healthy
    environment:
      AEGIS_HOST: "@@HOST@@"
      AEGIS_LISTEN: "@@LISTEN@@"
      AEGIS_CHECKOUT: "@@CHECKOUT@@"
      AEGIS_CONTROLLER: "@@CONTROLLER@@"
      AEGIS_RESERVED: "@@RESERVED@@"
      SOPS_AGE_KEY_FILE: "@@KEY_IN_CONTAINER@@"
      XDG_RUNTIME_DIR: "@@RUNTIME@@"
    volumes:
      - "/var/run/docker.sock:/var/run/docker.sock"
      - "@@CHECKOUT@@:@@CHECKOUT@@"
      - "@@KEY@@:@@KEY_IN_CONTAINER@@:ro"
      - "@@RUNTIME@@:@@RUNTIME@@"
      - "socket:/var/run/tailscale"
"#;

const CONTROLLER: &str = r#"  controller:
    image: "@@IMAGE@@"
    restart: unless-stopped
    network_mode: "service:tailscale"
    depends_on:
      tailscale:
        condition: service_healthy
    env_file:
      - "@@ENV_FILE@@"
    environment:
      AEGIS_LISTEN: "@@LISTEN@@"
      AEGIS_ENROLL_TAGS: "@@TAGS@@"
      AEGIS_REPO_URL: "@@REPO@@"
      AEGIS_WORKDIR: "@@WORKDIR@@"
      AEGIS_AGENTS: "@@AGENTS@@"
    volumes:
      - "socket:/var/run/tailscale"
"#;

const VOLUMES: &str = r#"volumes:
  "@@STATE_VOLUME@@": {}
  socket: {}
"#;

/// Which stack a file belongs to, its Compose project, and the role of its node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    name: StackName,
    project: ProjectName,
    role: AegisRole,
}

/// How a planned file gets its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    /// The text as it is.
    Plain(FileText),
    /// The dotenv text of this secret, encrypted to the host's age key first.
    Encrypted(aegis_domain::Secret),
}

/// A file of a stack and what to do when it exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFile {
    path: RepoPath,
    content: Content,
    existing: Existing,
}

impl PlannedFile {
    /// Where the file goes, relative to the bootstrap directory.
    #[must_use]
    pub const fn path(&self) -> &RepoPath {
        &self.path
    }

    /// How the file gets its text.
    #[must_use]
    pub const fn content(&self) -> &Content {
        &self.content
    }

    /// What to do when the file already exists.
    #[must_use]
    pub const fn existing(&self) -> Existing {
        self.existing
    }
}

/// The files of one stack.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlannedFiles(Vec<PlannedFile>);

impl PlannedFiles {
    /// The files in order.
    pub fn iter(&self) -> std::slice::Iter<'_, PlannedFile> {
        self.0.iter()
    }
}

impl<'items> IntoIterator for &'items PlannedFiles {
    type Item = &'items PlannedFile;
    type IntoIter = std::slice::Iter<'items, PlannedFile>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

/// A stack to install: who it is and what it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedStack {
    identity: Identity,
    files: PlannedFiles,
}

impl RenderedStack {
    /// The name of the stack.
    #[must_use]
    pub const fn name(&self) -> &StackName {
        &self.identity.name
    }

    /// The Compose project of the stack, which names its sidecar's state volume.
    #[must_use]
    pub const fn project(&self) -> &ProjectName {
        &self.identity.project
    }

    /// The role of the node.
    #[must_use]
    pub const fn role(&self) -> AegisRole {
        self.identity.role
    }

    /// The files of the stack.
    pub fn files(&self) -> std::slice::Iter<'_, PlannedFile> {
        self.files.iter()
    }
}

/// The stacks to install, in order: the controller first, so the agent can report to it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stacks(Vec<RenderedStack>);

impl IntoIterator for Stacks {
    type Item = RenderedStack;
    type IntoIter = std::vec::IntoIter<RenderedStack>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// `value` as the inside of a YAML double-quoted scalar. A `$` is doubled so Compose never
/// interpolates it.
fn escaped(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "$$")
}

/// A template whose `@@NAME@@` markers are replaced, each value escaped for a YAML scalar.
struct Template(String);

impl Template {
    const fn new(text: String) -> Self {
        Self(text)
    }

    fn with(self, name: &str, value: &str) -> Self {
        Self(self.0.replace(&format!("@@{name}@@"), &escaped(value)))
    }
}

/// A host path that Compose's short volume syntax cannot misread.
fn mountable(path: &HostPath) -> Result<&str, ValueError> {
    let forbidden = path
        .as_str()
        .chars()
        .find(|found| matches!(found, ':' | '\n'));
    forbidden.map_or(Ok(path.as_str()), |found| {
        Err(ValueError::ForbiddenChar(found))
    })
}

fn sidecar(node: &HostName) -> String {
    let template = Template::new(SIDECAR.to_owned());
    let filled = template
        .with("TAILSCALE", TAILSCALE_IMAGE)
        .with("NODE", node.as_str())
        .with("KEY", KEY_VARIABLE)
        .with("STATE_DIRECTORY", STATE_DIRECTORY)
        .with("STATE_VOLUME", STATE_VOLUME);
    filled.0
}

fn volumes() -> String {
    Template::new(VOLUMES.to_owned())
        .with("STATE_VOLUME", STATE_VOLUME)
        .0
}

fn image(name: &str, tag: &ImageTag) -> String {
    format!("{name}:{tag}")
}

fn agent_text(plan: &BootstrapPlan, key: &HostKey) -> Result<String, ValueError> {
    let sources = plan.sources();
    let reserved = match plan.controller() {
        ControllerChoice::Skip => "agent",
        ControllerChoice::Install(_) => "agent,controller",
    };
    let filled = Template::new(AGENT.to_owned())
        .with("IMAGE", &image(AGENT_IMAGE, plan.image()))
        .with("HOST", plan.host().as_str())
        .with("LISTEN", LISTEN)
        .with("CHECKOUT", mountable(sources.checkout())?)
        .with("CONTROLLER", sources.controller().as_str())
        .with("RESERVED", reserved)
        .with("KEY", mountable(key.path())?)
        .with("KEY_IN_CONTAINER", KEY_IN_CONTAINER)
        .with("RUNTIME", RUNTIME_DIRECTORY);
    Ok(filled.0)
}

fn agent_document(
    plan: &BootstrapPlan,
    key: &HostKey,
    node: &HostName,
) -> Result<String, ValueError> {
    let head = format!("services:\n{}{}", sidecar(node), agent_text(plan, key)?);
    Ok(format!("{head}{}", volumes()))
}

/// The `AEGIS_AGENTS` list: the agent bootstrap installs on this host, reached by its tailnet
/// name, then the ones of other hosts.
fn agents_of(plan: &BootstrapPlan, setup: &ControllerSetup) -> Result<String, ValueError> {
    let agent = node_of(plan, &StackName::new("agent")?)?;
    let url = AgentUrl::new(format!("http://{agent}:{PORT}"))?;
    let local = AgentEndpoint::new(plan.host().clone(), url);
    let others = setup.agents().into_iter();
    let entries = std::iter::once(&local)
        .chain(others)
        .map(AgentEndpoint::entry);
    Ok(entries.collect::<Vec<_>>().join(","))
}

fn controller_document(
    plan: &BootstrapPlan,
    setup: &ControllerSetup,
    node: &HostName,
) -> Result<String, ValueError> {
    let filled = Template::new(CONTROLLER.to_owned())
        .with("IMAGE", &image(CONTROLLER_IMAGE, plan.image()))
        .with("ENV_FILE", CONTROLLER_ENV)
        .with("TAGS", &setup.enroll_tags())
        .with("REPO", plan.sources().repo().as_str())
        .with("WORKDIR", CONTROLLER_WORKDIR)
        .with("AGENTS", &agents_of(plan, setup)?)
        .with("LISTEN", LISTEN);
    let head = format!("services:\n{}{}", sidecar(node), filled.0);
    Ok(format!("{head}{}", volumes()))
}

fn compose_file(directory: &str, text: String) -> Result<PlannedFile, ValueError> {
    Ok(PlannedFile {
        path: RepoPath::new(format!("{directory}/compose.yaml"))?,
        content: Content::Plain(FileText::new(text)),
        existing: Existing::Replace,
    })
}

fn node_of(plan: &BootstrapPlan, stack: &StackName) -> Result<HostName, ValueError> {
    HostName::new(format!("{}-aegis-{stack}", plan.host()))
}

fn agent_stack(plan: &BootstrapPlan, key: &HostKey) -> Result<RenderedStack, ValueError> {
    let name = StackName::new("agent")?;
    let node = node_of(plan, &name)?;
    let file = compose_file("aegis/agent", agent_document(plan, key, &node)?)?;
    let identity = Identity {
        project: ProjectName::of(&name)?,
        name,
        role: AegisRole::Agent,
    };
    Ok(RenderedStack {
        identity,
        files: PlannedFiles(vec![file]),
    })
}

fn controller_stack(
    plan: &BootstrapPlan,
    setup: &ControllerSetup,
) -> Result<RenderedStack, ValueError> {
    let name = StackName::new("controller")?;
    let node = node_of(plan, &name)?;
    let compose = compose_file("aegis/controller", controller_document(plan, setup, &node)?)?;
    let env = PlannedFile {
        path: RepoPath::new(format!("aegis/controller/{CONTROLLER_ENV}"))?,
        content: Content::Encrypted(setup.credentials().clone()),
        existing: Existing::Keep,
    };
    let identity = Identity {
        project: ProjectName::of(&name)?,
        name,
        role: AegisRole::Controller,
    };
    Ok(RenderedStack {
        identity,
        files: PlannedFiles(vec![compose, env]),
    })
}

/// The stacks `plan` installs.
///
/// # Errors
///
/// Returns a [`ValueError`] when a value derived from the plan cannot be used in a compose file.
pub fn stacks(plan: &BootstrapPlan, key: &HostKey) -> Result<Stacks, ValueError> {
    let mut rendered = Vec::new();
    if let ControllerChoice::Install(setup) = plan.controller() {
        rendered.push(controller_stack(plan, setup)?);
    }
    rendered.push(agent_stack(plan, key)?);
    Ok(Stacks(rendered))
}

#[cfg(test)]
mod tests {
    use super::escaped;

    #[test]
    fn a_scalar_escapes_quotes_backslashes_and_interpolation() {
        assert_eq!(escaped(r#"a"b\c$d"#), r#"a\"b\\c$$d"#);
    }
}
