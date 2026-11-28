//! The tailscale sidecar and Caddy that Aegis adds to a stack that exposes a service.
//!
//! The rendering is pure text with no secret in it: the auth key travels in an environment
//! variable.

use std::fmt;

use aegis_domain::ValueError;
use aegis_domain::{Expose, NodeLabel, Port, ProjectName, StackPolicy, TailnetName, TailnetTags};

/// The name of the environment variable that carries the minted auth key to Compose.
pub const KEY_VARIABLE: &str = "AEGIS_TS_AUTHKEY";

/// The image of the tailscale sidecar. The agent also runs it to look into the state volume.
pub const TAILSCALE_IMAGE: &str = "tailscale/tailscale:v1.80.2";

/// The image of the reverse proxy.
const CADDY_IMAGE: &str = "caddy:2.9.1";

/// The compose name of the volume that keeps the node identity. Compose prefixes it with the
/// project name.
pub const STATE_VOLUME: &str = "tailscale-state";

/// Where the node identity lives inside the state volume.
pub const STATE_FILE: &str = "tailscaled.state";

/// The directory of the state volume inside the sidecar.
pub const STATE_DIRECTORY: &str = "/var/lib/tailscale";

/// The Compose volume of `project` that keeps the identity of its tailnet node.
#[must_use]
pub fn state_volume_of(project: &ProjectName) -> String {
    format!("{project}_{STATE_VOLUME}")
}

/// What a stack's `aegis.toml` asks of the tailnet: the service to expose and the ACL tags of its
/// node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exposure {
    expose: Expose,
    tags: TailnetTags,
}

impl Exposure {
    /// Expose `expose`, with its node tagged `tags`.
    #[must_use]
    pub const fn new(expose: Expose, tags: TailnetTags) -> Self {
        Self { expose, tags }
    }

    /// The exposure `policy` asks for, if it asks for one.
    #[must_use]
    pub fn of(policy: &StackPolicy) -> Option<Self> {
        let expose = policy.expose()?;
        Some(Self::new(expose.clone(), policy.tailnet_tags().clone()))
    }

    /// The service and port to expose.
    #[must_use]
    pub const fn expose(&self) -> &Expose {
        &self.expose
    }

    /// The ACL tags of the node.
    #[must_use]
    pub const fn tags(&self) -> &TailnetTags {
        &self.tags
    }

    /// The hostname of the node: the exposed service's name.
    ///
    /// # Errors
    ///
    /// Returns a [`ValueError`] when the service name is not a valid DNS label, for example
    /// because it holds an underscore or an upper-case letter.
    pub fn node(&self) -> Result<NodeLabel, ValueError> {
        NodeLabel::new(self.expose.service().as_str())
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
    caddyfile: Caddyfile,
}

impl RenderedSidecar {
    /// The compose override. It holds the Caddyfile as an inline compose `config`.
    #[must_use]
    pub const fn compose(&self) -> &OverrideYaml {
        &self.compose
    }

    /// The Caddyfile, as it appears inside the override.
    #[must_use]
    pub const fn caddyfile(&self) -> &Caddyfile {
        &self.caddyfile
    }
}

/// Everything needed to render the sidecar of one exposed service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarSpec {
    node: NodeLabel,
    upstream: Expose,
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
            upstream: exposure.expose().clone(),
            tailnet,
        })
    }

    /// The address the service is served at, such as `vaultwarden.taildfab0.ts.net`.
    #[must_use]
    pub fn address(&self) -> String {
        format!("{}.{}", self.node, self.tailnet)
    }

    /// The Caddyfile: terminate TLS for the node's address and proxy to the service.
    #[must_use]
    pub fn caddyfile(&self) -> Caddyfile {
        let service = self.upstream.service();
        let port: Port = self.upstream.port();
        Caddyfile(format!(
            "{} {{\n  reverse_proxy {service}:{port}\n}}\n",
            self.address()
        ))
    }

    /// The override and Caddyfile for a node that `key` says whether to enrol. A deployment that
    /// shows the override to a person (`aegis diff`) calls this too.
    #[must_use]
    pub fn render(&self, key: KeyUse) -> RenderedSidecar {
        let caddyfile = self.caddyfile();
        let compose = OverrideYaml(self.compose_text(key, &caddyfile));
        RenderedSidecar { compose, caddyfile }
    }

    fn compose_text(&self, key: KeyUse, caddyfile: &Caddyfile) -> String {
        let services = [self.tailscale_service(key), caddy_service()].concat();
        format!("services:\n{services}{}{VOLUMES}", config(caddyfile))
    }

    fn tailscale_service(&self, key: KeyUse) -> String {
        let node = &self.node;
        let key_line = match key {
            KeyUse::Fresh => format!("      TS_AUTHKEY: \"${{{KEY_VARIABLE}}}\"\n"),
            KeyUse::Reused => String::new(),
        };
        format!(
            "  tailscale:
    image: {TAILSCALE_IMAGE}
    hostname: {node}
    restart: unless-stopped
    environment:
      TS_HOSTNAME: {node}
      TS_STATE_DIR: {STATE_DIRECTORY}
      TS_SOCKET: {SOCKET_DIRECTORY}/tailscaled.sock
{key_line}    volumes:
      - {STATE_VOLUME}:{STATE_DIRECTORY}
      - {SOCKET_VOLUME}:{SOCKET_DIRECTORY}
{TUN_ACCESS}"
        )
    }
}

/// The directory of the tailscale socket, shared with Caddy.
const SOCKET_DIRECTORY: &str = "/var/run/tailscale";

const SOCKET_VOLUME: &str = "tailscale-socket";

/// Caddy listens on the tailnet address, so the sidecar needs the TUN device and `NET_ADMIN`
/// (the incumbent hand-written sidecar does too). The override is generated from typed values and
/// is not checked by the compose allowlist, which excludes `devices`; revisiting both is #70.
const TUN_ACCESS: &str = "    devices:
      - /dev/net/tun:/dev/net/tun
    cap_add:
      - net_admin
";

const VOLUMES: &str = "volumes:
  tailscale-state:
  tailscale-socket:
";

fn caddy_service() -> String {
    format!(
        "  caddy:
    image: {CADDY_IMAGE}
    restart: unless-stopped
    network_mode: service:tailscale
    depends_on:
      - tailscale
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
