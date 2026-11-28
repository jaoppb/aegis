//! The Tailscale adapter against the real tailnet. It is ignored: it creates and deletes real
//! nodes, so it runs only when asked for.
//!
//! ```text
//! AEGIS_TS_CLIENT_ID=... AEGIS_TS_CLIENT_SECRET=... \
//!     cargo test -p aegis-adapters --test tailscale_live -- --ignored --nocapture
//! ```
//!
//! The OAuth client needs the write scopes `auth_keys` and `devices:core`, restricted to the test
//! tag (`tag:aegis-test`, or `AEGIS_TS_TEST_TAG`), and the tailnet policy must list that tag in
//! `tagOwners`. Docker must be running: the nodes are throwaway `tailscale/tailscale` containers.
//! Set `AEGIS_TS_PERSONAL_HOSTNAME` to the hostname of one of your own untagged devices to also
//! check that it is never matched. Nothing here prints the secret or a key.

use std::future::{Future, ready};
use std::process::Command;

use aegis_application::{Reason, SidecarState, StateVolume};
use aegis_domain::HostName;

struct Volume(StateVolume);

impl SidecarState for Volume {
    fn volume(&self, _node: &HostName) -> impl Future<Output = Result<StateVolume, Reason>> + Send {
        ready(Ok(self.0))
    }
}

/// A throwaway container and its state volume, removed when it goes out of scope.
struct Node(String);

impl Drop for Node {
    fn drop(&mut self) {
        drop(Command::new("docker").args(["rm", "-f", &self.0]).output());
        let volume = format!("{}-state", self.0);
        drop(
            Command::new("docker")
                .args(["volume", "rm", &volume])
                .output(),
        );
    }
}

#[cfg(test)]
mod cases {
    use std::process::Command;
    use std::time::Duration;

    use super::{Node, Volume};
    use aegis_adapters::tailscale::{ClientId, ClientSecret, OAuthCredentials, TailscaleApi};
    use aegis_application::{
        AuthKey, DeviceId, DeviceRef, Enrolled, Enrollment, KeyMinter, NodeRemover, StateVolume,
        TailnetError,
    };
    use aegis_domain::{HostName, TailnetTag, TailnetTags};
    use serde_json::Value;
    use tokio::runtime::Runtime;

    const POLLS: usize = 30;
    const PAUSE: Duration = Duration::from_secs(2);

    fn variable(name: &str) -> String {
        std::env::var(name).unwrap_or_else(|_| panic!("set {name}"))
    }

    fn api() -> TailscaleApi {
        let client = ClientId::new(variable("AEGIS_TS_CLIENT_ID"));
        let secret = ClientSecret::new(variable("AEGIS_TS_CLIENT_SECRET"));
        TailscaleApi::new(OAuthCredentials::new(client, secret)).unwrap()
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

    /// Start a throwaway node that joins the tailnet with `key`. The key goes to docker through
    /// the environment of the child process, never through an argument.
    fn start(name: &str, key: &AuthKey) -> Node {
        let volume = format!("{name}-state:/var/lib/tailscale");
        let status = Command::new("docker")
            .args([
                "run",
                "-d",
                "--name",
                name,
                "--hostname",
                name,
                "-v",
                &volume,
            ])
            .args(["-e", "TS_AUTHKEY", "-e", "TS_USERSPACE=true"])
            .args(["tailscale/tailscale:latest"])
            .env("TS_AUTHKEY", key.expose())
            .output()
            .unwrap();
        assert!(status.status.success(), "docker run failed");
        Node(name.to_owned())
    }

    fn status_json(name: &str) -> Option<Value> {
        let text = docker(&["exec", name, "tailscale", "status", "--json"])?;
        serde_json::from_str(&text).ok()
    }

    fn is_running(name: &str) -> bool {
        let state = status_json(name);
        state.is_some_and(|json| json["BackendState"] == "Running")
    }

    fn pause() -> bool {
        std::thread::sleep(PAUSE);
        false
    }

    fn joins(name: &str) -> bool {
        (0..POLLS).any(|_| is_running(name) || pause())
    }

    fn node_id(name: &str) -> String {
        let json = status_json(name).unwrap();
        json["Self"]["ID"].as_str().unwrap().to_owned()
    }

    fn mint(runtime: &Runtime, api: &TailscaleApi) -> AuthKey {
        let key = runtime.block_on(api.mint(&tags())).unwrap();
        assert!(key.expose().starts_with("tskey-auth-"));
        key
    }

    fn removed(runtime: &Runtime, api: &TailscaleApi, node: &DeviceRef) -> bool {
        let gone = |_: usize| runtime.block_on(api.remove(node)) == Err(TailnetError::NoSuchDevice);
        (0..POLLS).any(|attempt| gone(attempt) || pause())
    }

    fn enroll(runtime: &Runtime, api: TailscaleApi, volume: StateVolume) -> Enrolled {
        let node = HostName::new("aegis-live-state").unwrap();
        let enrollment = Enrollment::new(Volume(volume), api);
        runtime.block_on(enrollment.enroll(&node, &tags())).unwrap()
    }

    #[test]
    #[ignore = "talks to the real tailnet: needs OAuth credentials and docker"]
    fn mint_join_reuse_enroll_and_delete() {
        let runtime = Runtime::new().unwrap();
        let api = api();
        let key = mint(&runtime, &api);
        let first = start("aegis-live-first", &key);
        assert!(joins(&first.0), "the node never joined the tailnet");
        let second = start("aegis-live-second", &key);
        assert!(!joins(&second.0), "a single-use key joined a second node");
        let populated = enroll(&runtime, api.clone(), StateVolume::Populated);
        assert_eq!(populated, Enrolled::AlreadyEnrolled);
        let empty = enroll(&runtime, api.clone(), StateVolume::Empty);
        assert!(matches!(empty, Enrolled::Minted(_)));
        delete_by_hostname(&runtime, &api, &first);
    }

    fn delete_by_hostname(runtime: &Runtime, api: &TailscaleApi, first: &Node) {
        let target = by_hostname(&first.0);
        runtime.block_on(api.remove(&target)).unwrap();
        assert!(removed(runtime, api, &target), "the node is still listed");
    }

    #[test]
    #[ignore = "talks to the real tailnet: needs OAuth credentials and docker"]
    fn delete_by_id_and_never_a_personal_device() {
        let runtime = Runtime::new().unwrap();
        let api = api();
        let node = start("aegis-live-by-id", &mint(&runtime, &api));
        assert!(joins(&node.0), "the node never joined the tailnet");
        let target = DeviceRef::Identifier(DeviceId::new(node_id(&node.0)).unwrap());
        runtime.block_on(api.remove(&target)).unwrap();
        assert!(removed(&runtime, &api, &target), "the node is still listed");
        personal_device_is_never_matched(&runtime, &api);
    }

    fn personal_device_is_never_matched(runtime: &Runtime, api: &TailscaleApi) {
        let personal = std::env::var("AEGIS_TS_PERSONAL_HOSTNAME");
        let results = personal
            .iter()
            .map(|name| runtime.block_on(api.remove(&by_hostname(name))));
        assert!(
            results
                .into_iter()
                .all(|result| result == Err(TailnetError::NoSuchDevice))
        );
        println!("untagged check ran: {}", personal.is_ok());
    }
}
