//! The Tailscale adapter against the real tailnet. It is ignored: it creates and deletes real
//! nodes, so it runs only when asked for, never in CI.
//!
//! ```text
//! AEGIS_TS_CLIENT_ID=... AEGIS_TS_CLIENT_SECRET=... AEGIS_TS_PERSONAL_HOSTNAME=<untagged device> \
//!     cargo test -p aegis-adapters --test tailscale_live -- --ignored --nocapture
//! ```
//!
//! Needs:
//! - an OAuth client with the write scopes `auth_keys` and `devices:core`, restricted to the test
//!   tag (`tag:aegis-test`, or `AEGIS_TS_TEST_TAG`) that the tailnet policy lists in `tagOwners`;
//! - Docker: the nodes are throwaway `tailscale/tailscale` containers (`AEGIS_TS_IMAGE` overrides
//!   the pinned image);
//! - for the personal-device test, `AEGIS_TS_PERSONAL_HOSTNAME`: the hostname of one of YOUR OWN
//!   UNTAGGED devices, which is on this machine's tailnet view. The test refuses to run when that
//!   device is tagged or not found, because it asks the adapter to delete it by hostname and the
//!   adapter must refuse.
//!
//! A run takes one to three minutes. Every node gets a hostname unique to the run
//! (`aegis-live-<label>-<pid>`), so runs never collide; a guard removes the container, its volume
//! and the tailnet device even when an assertion fails. A run that is killed skips the guard: clean
//! up by hand with `docker rm -f` of the `aegis-live-*` containers, `docker volume rm` of their
//! `*-state` volumes, and by deleting the `aegis-live-*` devices in the admin console. Nothing here
//! prints the secret or a key.

use std::future::{Future, ready};
use std::process::Command;
use std::sync::Arc;

use aegis_adapters::tailscale::TailscaleApi;
use aegis_application::{NodeRemover, Reason, SidecarState, StateVolume};
use aegis_domain::HostName;
use tokio::runtime::Runtime;

struct Volume(StateVolume);

impl SidecarState for Volume {
    fn volume(&self, _node: &HostName) -> impl Future<Output = Result<StateVolume, Reason>> + Send {
        ready(Ok(self.0))
    }
}

/// The runtime and the API client the nodes are cleaned up with.
#[derive(Clone)]
struct Lab {
    runtime: Arc<Runtime>,
    api: TailscaleApi,
}

/// A throwaway node: its container, state volume and tailnet device are removed on drop.
struct Node {
    name: String,
    lab: Lab,
}

impl Drop for Node {
    fn drop(&mut self) {
        drop(
            Command::new("docker")
                .args(["rm", "-f", &self.name])
                .output(),
        );
        let volume = format!("{}-state", self.name);
        drop(
            Command::new("docker")
                .args(["volume", "rm", &volume])
                .output(),
        );
        let host = HostName::new(self.name.as_str());
        let node = host.map(aegis_application::DeviceRef::Hostname);
        let removal = node.map(|device| self.lab.runtime.block_on(self.lab.api.remove(&device)));
        drop(removal);
    }
}

#[cfg(test)]
mod cases {
    use std::process::Command;
    use std::time::Duration;

    use super::{Lab, Node, Volume};
    use aegis_adapters::tailscale::{ClientId, ClientSecret, OAuthCredentials, TailscaleApi};
    use aegis_application::{
        AuthKey, DeviceId, DeviceRef, Enrolled, Enrollment, KeyMinter, NodeRemover, StateVolume,
        TailnetError,
    };
    use aegis_domain::{HostName, TailnetTag, TailnetTags};
    use serde_json::Value;
    use std::sync::Arc;
    use tokio::runtime::Runtime;

    const POLLS: usize = 30;
    const PAUSE: Duration = Duration::from_secs(2);
    const SETTLE: Duration = Duration::from_secs(20);
    const IMAGE: &str = "tailscale/tailscale:v1.104.6";

    fn variable(name: &str) -> String {
        std::env::var(name).unwrap_or_else(|_| panic!("set {name}"))
    }

    fn lab() -> Lab {
        let client = ClientId::new(variable("AEGIS_TS_CLIENT_ID"));
        let secret = ClientSecret::new(variable("AEGIS_TS_CLIENT_SECRET"));
        let api = TailscaleApi::new(OAuthCredentials::new(client, secret)).unwrap();
        let runtime = Arc::new(Runtime::new().unwrap());
        Lab { runtime, api }
    }

    fn tags() -> TailnetTags {
        let name = std::env::var("AEGIS_TS_TEST_TAG").unwrap_or_else(|_| "tag:aegis-test".into());
        std::iter::once(TailnetTag::new(name).unwrap()).collect()
    }

    fn by_hostname(name: &str) -> DeviceRef {
        DeviceRef::Hostname(HostName::new(name).unwrap())
    }

    fn docker(arguments: &[&str]) -> Option<String> {
        let output = Command::new("docker").args(arguments).output().ok()?;
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        output.status.success().then_some(text)
    }

    /// Start a throwaway node `aegis-live-<label>-<pid>` that joins the tailnet with `key`. The key
    /// goes to docker through the environment of the child process, never through an argument.
    fn start(lab: &Lab, label: &str, key: &AuthKey) -> Node {
        let name = format!("aegis-live-{label}-{}", std::process::id());
        let image = std::env::var("AEGIS_TS_IMAGE").unwrap_or_else(|_| IMAGE.into());
        let volume = format!("{name}-state:/var/lib/tailscale");
        let node = Node {
            name: name.clone(),
            lab: lab.clone(),
        };
        let started = Command::new("docker")
            .args([
                "run",
                "-d",
                "--name",
                &name,
                "--hostname",
                &name,
                "-v",
                &volume,
            ])
            .args(["-e", "TS_AUTHKEY", "-e", "TS_USERSPACE=true", &image])
            .env("TS_AUTHKEY", key.expose())
            .output()
            .unwrap();
        assert!(started.status.success(), "docker run failed");
        node
    }

    fn status_json(name: &str) -> Option<Value> {
        let text = docker(&["exec", name, "tailscale", "status", "--json"])?;
        serde_json::from_str(&text).ok()
    }

    fn backend_state(name: &str) -> Option<String> {
        let json = status_json(name)?;
        json["BackendState"].as_str().map(str::to_owned)
    }

    fn pause() -> bool {
        std::thread::sleep(PAUSE);
        false
    }

    fn joins(name: &str) -> bool {
        (0..POLLS).any(|_| backend_state(name).as_deref() == Some("Running") || pause())
    }

    /// The daemon of `name` answers but, after a grace period, has not logged in: the key it was
    /// given did not work.
    fn answers_but_never_logs_in(name: &str) -> bool {
        let daemon_up = (0..POLLS).any(|_| status_json(name).is_some() || pause());
        std::thread::sleep(SETTLE);
        daemon_up && backend_state(name).as_deref() == Some("NeedsLogin")
    }

    fn node_id(name: &str) -> String {
        let json = status_json(name).unwrap();
        json["Self"]["ID"].as_str().unwrap().to_owned()
    }

    fn mint(lab: &Lab) -> AuthKey {
        let key = lab.runtime.block_on(lab.api.mint(&tags())).unwrap();
        assert!(key.expose().starts_with("tskey-auth-"));
        key
    }

    fn is_gone(lab: &Lab, node: &DeviceRef) -> bool {
        let result = lab.runtime.block_on(lab.api.remove(node));
        result == Err(TailnetError::NoSuchDevice)
    }

    fn removed(lab: &Lab, node: &DeviceRef) -> bool {
        (0..POLLS).any(|_| is_gone(lab, node) || pause())
    }

    fn enroll(lab: &Lab, volume: StateVolume) -> Enrolled {
        let node = HostName::new("aegis-live-state").unwrap();
        let enrollment = Enrollment::new(Volume(volume), lab.api.clone());
        lab.runtime
            .block_on(enrollment.enroll(&node, &tags()))
            .unwrap()
    }

    fn delete_by_hostname(lab: &Lab, node: &Node) {
        let target = by_hostname(&node.name);
        lab.runtime.block_on(lab.api.remove(&target)).unwrap();
        assert!(removed(lab, &target), "the node is still listed");
    }

    #[test]
    #[ignore = "talks to the real tailnet: needs OAuth credentials and docker"]
    fn mint_join_reuse_enroll_and_delete_by_hostname() {
        let lab = lab();
        let key = mint(&lab);
        let first = start(&lab, "first", &key);
        assert!(joins(&first.name), "the node never joined the tailnet");
        let second = start(&lab, "second", &key);
        assert!(
            answers_but_never_logs_in(&second.name),
            "a single-use key must not log a second node in"
        );
        assert_eq!(
            enroll(&lab, StateVolume::Populated),
            Enrolled::AlreadyEnrolled
        );
        assert!(matches!(
            enroll(&lab, StateVolume::Empty),
            Enrolled::Minted(_)
        ));
        delete_by_hostname(&lab, &first);
    }

    #[test]
    #[ignore = "talks to the real tailnet: needs OAuth credentials and docker"]
    fn delete_by_id() {
        let lab = lab();
        let node = start(&lab, "by-id", &mint(&lab));
        assert!(joins(&node.name), "the node never joined the tailnet");
        let target = DeviceRef::Identifier(DeviceId::new(node_id(&node.name)).unwrap());
        lab.runtime.block_on(lab.api.remove(&target)).unwrap();
        assert!(removed(&lab, &target), "the node is still listed");
    }

    /// The `HostName`, tags and all, of every device this machine sees: itself and its peers.
    fn devices_seen_locally() -> Vec<Value> {
        let output = Command::new("tailscale")
            .args(["status", "--json"])
            .output()
            .unwrap();
        let json: Value = serde_json::from_slice(&output.stdout).unwrap();
        let peers = json["Peer"]
            .as_object()
            .into_iter()
            .flat_map(|map| map.values().cloned());
        std::iter::once(json["Self"].clone()).chain(peers).collect()
    }

    fn has_hostname(device: &Value, hostname: &str) -> bool {
        let found = device["HostName"].as_str();
        found.is_some_and(|name| name.eq_ignore_ascii_case(hostname))
    }

    /// Whether the local `tailscale` sees a device called `hostname` that carries a tag.
    fn is_tagged(hostname: &str) -> Option<bool> {
        let devices = devices_seen_locally();
        let found = devices
            .iter()
            .find(|device| has_hostname(device, hostname))?;
        Some(
            found["Tags"]
                .as_array()
                .is_some_and(|tags| !tags.is_empty()),
        )
    }

    #[test]
    #[ignore = "talks to the real tailnet: needs OAuth credentials and AEGIS_TS_PERSONAL_HOSTNAME"]
    fn an_untagged_personal_device_is_never_deleted() {
        let lab = lab();
        let hostname = variable("AEGIS_TS_PERSONAL_HOSTNAME");
        let tagged = is_tagged(&hostname);
        assert_eq!(
            tagged,
            Some(false),
            "{hostname} must be an untagged device this machine sees"
        );
        let result = lab
            .runtime
            .block_on(lab.api.remove(&by_hostname(&hostname)));
        assert_eq!(result, Err(TailnetError::NoSuchDevice));
    }
}
