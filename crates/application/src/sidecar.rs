//! The tailscale sidecar and Caddy that Aegis adds to a stack that exposes a service.
//!
//! The rendering is pure text with no secret in it: the auth key travels in an environment
//! variable.

use std::fmt;

use aegis_domain::ValueError;
use aegis_domain::{
    Expose, Node, NodeLabel, Port, ProjectName, ServiceName, StackPolicy, TailnetName, TailnetTags,
};

/// The name of the environment variable that carries the minted auth key to Compose.
pub const KEY_VARIABLE: &str = "AEGIS_TS_AUTHKEY";

/// The image of the tailscale sidecar. The agent also runs it to look into the state volume.
pub const TAILSCALE_IMAGE: &str = "tailscale/tailscale:v1.80.2";

/// The image of the reverse proxy.
const CADDY_IMAGE: &str = "caddy:2.9.1";

/// The compose service that runs tailscale.
pub const TAILSCALE_SERVICE: &str = "tailscale";

/// The compose service that runs the reverse proxy.
pub const CADDY_SERVICE: &str = "caddy";

/// The compose name of the volume that keeps the node identity. Compose prefixes it with the
/// project name.
pub const STATE_VOLUME: &str = "tailscale-state";

/// Where the node identity lives inside the state volume.
pub const STATE_FILE: &str = "tailscaled.state";

/// The directory of the state volume inside the sidecar.
pub const STATE_DIRECTORY: &str = "/var/lib/tailscale";

/// Whether `name` is a compose service Aegis injects for an exposed stack.
#[must_use]
pub fn is_sidecar_service(name: &str) -> bool {
    name == TAILSCALE_SERVICE || name == CADDY_SERVICE
}

/// Whether `name` is a compose volume Aegis injects for an exposed stack.
#[must_use]
pub fn is_sidecar_volume(name: &str) -> bool {
    name == STATE_VOLUME || name == SOCKET_VOLUME
}

/// The Compose volume of `project` that keeps the identity of its tailnet node.
#[must_use]
pub fn state_volume_of(project: &ProjectName) -> String {
    format!("{project}_{STATE_VOLUME}")
}

/// How the node of a stack serves its service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Serving {
    /// HTTPS, through Caddy.
    Https(Expose),
    /// Raw TCP and UDP: the service shares the node's network namespace and no Caddy runs.
    Raw(Node),
}

impl Serving {
    const fn https(&self) -> Option<&Expose> {
        match self {
            Self::Https(expose) => Some(expose),
            Self::Raw(_) => None,
        }
    }

    /// The compose service the node serves.
    #[must_use]
    pub const fn service(&self) -> &ServiceName {
        match self {
            Self::Https(expose) => expose.service(),
            Self::Raw(node) => node.service(),
        }
    }
}

/// What a stack's `aegis.toml` asks of the tailnet: how to serve a service and the ACL tags of its
/// node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exposure {
    serving: Serving,
    tags: TailnetTags,
}

impl Exposure {
    /// Expose `expose` over HTTPS, with its node tagged `tags`.
    #[must_use]
    pub const fn new(expose: Expose, tags: TailnetTags) -> Self {
        Self {
            serving: Serving::Https(expose),
            tags,
        }
    }

    /// Serve raw TCP and UDP of `node`'s service, with its node tagged `tags`.
    #[must_use]
    pub const fn raw(node: Node, tags: TailnetTags) -> Self {
        Self {
            serving: Serving::Raw(node),
            tags,
        }
    }

    /// The exposure `policy` asks for, if it asks for one.
    #[must_use]
    pub fn of(policy: &StackPolicy) -> Option<Self> {
        let tags = policy.tailnet_tags().clone();
        let https = policy
            .expose()
            .map(|expose| Self::new(expose.clone(), tags.clone()));
        https.or_else(|| policy.node().map(|node| Self::raw(node.clone(), tags)))
    }

    /// How the node serves its service.
    #[must_use]
    pub const fn serving(&self) -> &Serving {
        &self.serving
    }

    /// The ACL tags of the node.
    #[must_use]
    pub const fn tags(&self) -> &TailnetTags {
        &self.tags
    }

    /// The hostname of the node: the served service's name.
    ///
    /// # Errors
    ///
    /// Returns a [`ValueError`] when the service name is not a valid DNS label, for example
    /// because it holds an underscore or an upper-case letter.
    pub fn node(&self) -> Result<NodeLabel, ValueError> {
        NodeLabel::new(self.serving.service().as_str())
    }
}

/// Whether the sidecar needs an auth key to join the tailnet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyUse {
    /// The node has never joined: the sidecar reads a freshly minted key from [`KEY_VARIABLE`].
    Fresh,
    /// The node keeps its identity in the state volume, so no key is given.
    Reused,
}

/// The compose override text.
#[derive(Clone, PartialEq, Eq)]
pub struct OverrideYaml(String);

impl OverrideYaml {
    /// The text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for OverrideYaml {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OverrideYaml(..)")
    }
}

/// The Caddyfile text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caddyfile(String);

impl Caddyfile {
    /// The text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What Aegis adds to a stack that exposes a service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedSidecar {
    compose: OverrideYaml,
    caddyfile: Option<Caddyfile>,
}

impl RenderedSidecar {
    /// The compose override. For an HTTPS exposure it holds the Caddyfile as an inline compose
    /// `config`.
    #[must_use]
    pub const fn compose(&self) -> &OverrideYaml {
        &self.compose
    }

    /// The Caddyfile, as it appears inside the override; a raw node runs no Caddy and has none.
    #[must_use]
    pub const fn caddyfile(&self) -> Option<&Caddyfile> {
        self.caddyfile.as_ref()
    }
}

/// Everything needed to render the sidecar of one exposed service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarSpec {
    node: NodeLabel,
    serving: Serving,
    tailnet: TailnetName,
}

impl SidecarSpec {
    /// The sidecar that serves `exposure` as a node of `tailnet`.
    ///
    /// # Errors
    ///
    /// Returns a [`ValueError`] when the exposed service's name is not a valid hostname.
    pub fn new(exposure: &Exposure, tailnet: TailnetName) -> Result<Self, ValueError> {
        Ok(Self {
            node: exposure.node()?,
            serving: exposure.serving().clone(),
            tailnet,
        })
    }

    /// The address the service is served at, such as `vaultwarden.taildfab0.ts.net`.
    #[must_use]
    pub fn address(&self) -> String {
        format!("{}.{}", self.node, self.tailnet)
    }

    /// The Caddyfile: terminate TLS for the node's address and proxy to the service. A raw node
    /// has none.
    #[must_use]
    pub fn caddyfile(&self) -> Option<Caddyfile> {
        let upstream = self.serving.https()?;
        let service = upstream.service();
        let port: Port = upstream.port();
        Some(Caddyfile(format!(
            "{} {{\n  reverse_proxy {service}:{port}\n}}\n",
            self.address()
        )))
    }

    /// The override and Caddyfile for a node that `key` says whether to enrol. A deployment that
    /// shows the override to a person (`aegis diff`) calls this too.
    #[must_use]
    pub fn render(&self, key: KeyUse) -> RenderedSidecar {
        let caddyfile = self.caddyfile();
        let compose = OverrideYaml(self.compose_text(key, caddyfile.as_ref()));
        RenderedSidecar { compose, caddyfile }
    }

    fn compose_text(&self, key: KeyUse, caddyfile: Option<&Caddyfile>) -> String {
        caddyfile.map_or_else(|| self.raw_text(key), |found| self.https_text(key, found))
    }

    fn https_text(&self, key: KeyUse, caddyfile: &Caddyfile) -> String {
        let tailscale = self.tailscale_service(key, Socket::Shared);
        let services = [tailscale, caddy_service()].concat();
        format!(
            "services:\n{services}{}volumes:\n  {STATE_VOLUME}:\n  {SOCKET_VOLUME}:\n",
            config(caddyfile)
        )
    }

    /// The override of a raw node: the sidecar, and the service joined to its network namespace.
    fn raw_text(&self, key: KeyUse) -> String {
        let tailscale = self.tailscale_service(key, Socket::Absent);
        let service = self.serving.service();
        format!(
            "services:\n{tailscale}  {service}:
    network_mode: service:{TAILSCALE_SERVICE}
    depends_on:
      - {TAILSCALE_SERVICE}
volumes:\n  {STATE_VOLUME}:\n"
        )
    }

    fn tailscale_service(&self, key: KeyUse, socket: Socket) -> String {
        let node = &self.node;
        let key_line = match key {
            KeyUse::Fresh => format!("      TS_AUTHKEY: \"${{{KEY_VARIABLE}}}\"\n"),
            KeyUse::Reused => String::new(),
        };
        format!(
            "  {TAILSCALE_SERVICE}:
    image: {TAILSCALE_IMAGE}
    hostname: {node}
    restart: unless-stopped
    environment:
      TS_HOSTNAME: {node}
      TS_STATE_DIR: {STATE_DIRECTORY}
{}{key_line}    volumes:
      - {STATE_VOLUME}:{STATE_DIRECTORY}
{}{TUN_ACCESS}",
            socket.environment(),
            socket.volume(),
        )
    }
}

/// The directory of the tailscale socket, shared with Caddy.
const SOCKET_DIRECTORY: &str = "/var/run/tailscale";

/// The compose name of the volume that holds the tailscale socket, shared with Caddy.
pub const SOCKET_VOLUME: &str = "tailscale-socket";

/// Whether the sidecar shares its socket with Caddy.
#[derive(Debug, Clone, Copy)]
enum Socket {
    /// Caddy reads the socket from a shared volume.
    Shared,
    /// Nothing else reads it.
    Absent,
}

impl Socket {
    const fn environment(self) -> &'static str {
        match self {
            Self::Shared => "      TS_SOCKET: /var/run/tailscale/tailscaled.sock\n",
            Self::Absent => "",
        }
    }

    const fn volume(self) -> &'static str {
        match self {
            Self::Shared => "      - tailscale-socket:/var/run/tailscale\n",
            Self::Absent => "",
        }
    }
}

/// Caddy listens on the tailnet address, so the sidecar needs the TUN device and `NET_ADMIN`
/// (the incumbent hand-written sidecar does too). The override is generated from typed values and
/// is not checked by the compose allowlist, which excludes `devices`; revisiting both is #70.
const TUN_ACCESS: &str = "    devices:
      - /dev/net/tun:/dev/net/tun
    cap_add:
      - net_admin
";

fn caddy_service() -> String {
    format!(
        "  {CADDY_SERVICE}:
    image: {CADDY_IMAGE}
    restart: unless-stopped
    network_mode: service:{TAILSCALE_SERVICE}
    depends_on:
      - {TAILSCALE_SERVICE}
    volumes:
      - {SOCKET_VOLUME}:{SOCKET_DIRECTORY}:ro
    configs:
      - source: aegis-caddyfile
        target: /etc/caddy/Caddyfile
"
    )
}

/// The Caddyfile as an inline compose config, so it needs no file of its own.
fn config(caddyfile: &Caddyfile) -> String {
    let lines = caddyfile.as_str().lines();
    let content = lines.fold(String::new(), |mut text, line| {
        text.push_str("      ");
        text.push_str(line);
        text.push('\n');
        text
    });
    format!("configs:\n  aegis-caddyfile:\n    content: |\n{content}")
}
