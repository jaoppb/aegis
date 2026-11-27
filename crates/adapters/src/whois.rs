//! The `tailscale whois --json` adapter of the [`WhoIs`] port.

use std::process::Output;
use std::time::Duration;

use aegis_application::{PeerAddress, Reason, WhoIs, WhoIsError};
use aegis_domain::{PeerIdentity, TailnetTag, TailnetTags};
use serde::Deserialize;
use tokio::process::Command;

/// How long the CLI may take before the lookup counts as failed.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);

/// What `tailscale whois` prints when the address belongs to no node.
const NO_MATCH: &str = "no match";

/// Resolves peers by running the `tailscale` CLI, which asks the local daemon.
#[derive(Debug, Clone)]
pub struct TailscaleWhois {
    program: String,
}

impl TailscaleWhois {
    /// The adapter that runs `tailscale` from the `PATH`.
    #[must_use]
    pub fn new() -> Self {
        Self::with_program("tailscale")
    }

    fn with_program(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
        }
    }

    async fn run(&self, peer: &PeerAddress) -> Result<Output, WhoIsError> {
        let mut command = Command::new(&self.program);
        command.args(["whois", "--json"]).arg(peer.to_string());
        command.kill_on_drop(true);
        let lookup = tokio::time::timeout(LOOKUP_TIMEOUT, command.output());
        let finished = lookup.await.map_err(|elapsed| unavailable(&elapsed))?;
        finished.map_err(|failure| unavailable(&failure))
    }
}

impl Default for TailscaleWhois {
    fn default() -> Self {
        Self::new()
    }
}

impl WhoIs for TailscaleWhois {
    async fn identify(&self, peer: &PeerAddress) -> Result<PeerIdentity, WhoIsError> {
        let output = self.run(peer).await?;
        interpret(&output)
    }
}

fn interpret(output: &Output) -> Result<PeerIdentity, WhoIsError> {
    if output.status.success() {
        return parse_identity(&output.stdout);
    }
    Err(failure_of(&output.stderr))
}

fn unavailable(cause: &impl std::fmt::Display) -> WhoIsError {
    WhoIsError::Unavailable(Reason::new(cause))
}

fn failure_of(stderr: &[u8]) -> WhoIsError {
    let text = String::from_utf8_lossy(stderr);
    if text.contains(NO_MATCH) {
        return WhoIsError::Unknown;
    }
    unavailable(&text.trim())
}

#[derive(Deserialize)]
struct Reply {
    #[serde(rename = "Node")]
    node: Option<Node>,
}

#[derive(Deserialize)]
struct Node {
    #[serde(rename = "Tags")]
    tags: Option<Vec<String>>,
}

/// Read the identity out of the JSON `tailscale whois --json` printed. A node without tags (a
/// user-owned node) is an identity with no tags; a tag that is not a valid ACL tag can never match
/// a required one, so it is dropped.
fn parse_identity(json: &[u8]) -> Result<PeerIdentity, WhoIsError> {
    let reply: Reply = serde_json::from_slice(json).map_err(|failure| unavailable(&failure))?;
    let node = reply
        .node
        .ok_or_else(|| unavailable(&"the reply has no node"))?;
    let tags = node.tags.unwrap_or_default();
    let valid = tags.into_iter().filter_map(|tag| TailnetTag::new(tag).ok());
    Ok(PeerIdentity::new(valid.collect::<TailnetTags>()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_on<Output>(future: impl std::future::Future<Output = Output>) -> Output {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(future)
    }

    fn tag(name: &str) -> TailnetTag {
        TailnetTag::new(name).unwrap()
    }

    #[test]
    fn a_tagged_node_carries_its_tags() {
        let json = br#"{"Node":{"Name":"a.ts.net.","Tags":["tag:aegis-agent","tag:other"]},"UserProfile":{}}"#;
        let peer = parse_identity(json).unwrap();
        assert!(peer.has_tag(&tag("tag:aegis-agent")));
        assert!(peer.has_tag(&tag("tag:other")));
    }

    #[test]
    fn a_user_owned_node_has_no_tags() {
        let null_tags = parse_identity(br#"{"Node":{"Tags":null}}"#).unwrap();
        let no_tags = parse_identity(br#"{"Node":{}}"#).unwrap();
        assert_eq!(null_tags, PeerIdentity::default());
        assert_eq!(no_tags, PeerIdentity::default());
    }

    #[test]
    fn invalid_tags_are_dropped() {
        let peer = parse_identity(br#"{"Node":{"Tags":["aegis-agent"]}}"#).unwrap();
        assert_eq!(peer, PeerIdentity::default());
    }

    #[test]
    fn a_reply_without_a_node_is_a_failed_lookup() {
        assert!(matches!(
            parse_identity(b"{}"),
            Err(WhoIsError::Unavailable(_))
        ));
        assert!(matches!(
            parse_identity(b"not json"),
            Err(WhoIsError::Unavailable(_))
        ));
    }

    fn finished(code: i32, stdout: &[u8], stderr: &[u8]) -> Output {
        use std::os::unix::process::ExitStatusExt;
        Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.to_vec(),
            stderr: stderr.to_vec(),
        }
    }

    #[test]
    fn a_successful_exit_is_read_as_an_identity() {
        let output = finished(0, br#"{"Node":{"Tags":["tag:aegis-agent"]}}"#, b"");
        assert!(interpret(&output).unwrap().has_tag(&tag("tag:aegis-agent")));
    }

    #[test]
    fn a_failed_exit_ignores_stdout() {
        let tagged = br#"{"Node":{"Tags":["tag:aegis-agent"]}}"#;
        let unknown = finished(1, tagged, b"no match for 1.2.3.4");
        let broken = finished(1, tagged, b"daemon down");
        assert_eq!(interpret(&unknown), Err(WhoIsError::Unknown));
        assert!(matches!(
            interpret(&broken),
            Err(WhoIsError::Unavailable(_))
        ));
    }

    #[test]
    fn no_match_means_an_unknown_peer() {
        assert_eq!(failure_of(b"no match for 1.2.3.4\n"), WhoIsError::Unknown);
        assert!(matches!(
            failure_of(b"daemon down"),
            WhoIsError::Unavailable(_)
        ));
    }

    #[test]
    fn a_missing_program_is_a_failed_lookup() {
        let whois = TailscaleWhois::with_program("/nonexistent/aegis-tailscale");
        let peer = PeerAddress::new("100.64.0.1:1");
        let result = block_on(whois.identify(&peer));
        assert!(matches!(result, Err(WhoIsError::Unavailable(_))));
    }
}
