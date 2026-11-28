//! The JSON and form bodies of the Tailscale API.

use aegis_domain::{AegisRole, TailnetTag, TailnetTags};
use serde::{Deserialize, Serialize};

/// How long a minted key stays valid. It is single-use, so this only bounds an unused key.
const KEY_LIFETIME_SECONDS: u64 = 3600;

/// The answer of the token endpoint.
#[derive(Deserialize)]
pub(super) struct TokenReply {
    access_token: String,
}

impl TokenReply {
    pub(super) fn into_token(self) -> String {
        self.access_token
    }
}

/// A flag of a key request, written as a JSON boolean.
#[derive(Clone, Copy)]
enum Switch {
    Enabled,
    Disabled,
}

impl Serialize for Switch {
    fn serialize<Out: serde::Serializer>(&self, out: Out) -> Result<Out::Ok, Out::Error> {
        out.serialize_bool(matches!(self, Self::Enabled))
    }
}

/// The body of a key request: one single-use, pre-authorized, non-ephemeral, tagged key.
#[derive(Serialize)]
pub(super) struct KeyRequest {
    capabilities: Capabilities,
    #[serde(rename = "expirySeconds")]
    expiry_seconds: u64,
}

#[derive(Serialize)]
struct Capabilities {
    devices: Devices,
}

#[derive(Serialize)]
struct Devices {
    create: Create,
}

#[derive(Serialize)]
struct Create {
    reusable: Switch,
    ephemeral: Switch,
    preauthorized: Switch,
    tags: Vec<String>,
}

impl KeyRequest {
    /// The request for a key that tags its node with `tags`.
    pub(super) fn single_use(tags: &TailnetTags) -> Self {
        let create = Create {
            reusable: Switch::Disabled,
            ephemeral: Switch::Disabled,
            preauthorized: Switch::Enabled,
            tags: tags.iter().map(ToString::to_string).collect(),
        };
        Self {
            capabilities: Capabilities {
                devices: Devices { create },
            },
            expiry_seconds: KEY_LIFETIME_SECONDS,
        }
    }
}

/// The answer of the key endpoint.
#[derive(Deserialize)]
pub(super) struct KeyReply {
    key: String,
}

impl KeyReply {
    pub(super) fn into_key(self) -> String {
        self.key
    }
}

/// The answer of the device list endpoint.
#[derive(Deserialize)]
pub(super) struct DeviceList {
    devices: Vec<Device>,
}

#[derive(Deserialize)]
struct Device {
    #[serde(rename = "id")]
    identifier: String,
    hostname: Option<String>,
    tags: Option<Vec<String>>,
}

fn is_role_tag(tag: &str) -> bool {
    TailnetTag::new(tag).is_ok_and(|valid| AegisRole::is_role_tag(&valid))
}

impl Device {
    /// Whether this is a stack's node called `hostname`: only a tagged device qualifies, so a
    /// person's own machine that shares the name is never matched, and neither is a node of Aegis
    /// itself (the controller or an agent). Case is ignored, as in DNS.
    fn is_named(&self, hostname: &str) -> bool {
        let tagged = self.tags.as_ref().is_some_and(|tags| !tags.is_empty());
        let aegis = self.tags.iter().flatten().any(|tag| is_role_tag(tag));
        let found = self.hostname.as_deref();
        tagged && !aegis && found.is_some_and(|name| name.eq_ignore_ascii_case(hostname))
    }
}

impl DeviceList {
    /// The ids of the tagged devices whose hostname is `hostname`.
    pub(super) fn ids_of(&self, hostname: &str) -> Vec<&str> {
        let matching = self
            .devices
            .iter()
            .filter(|device| device.is_named(hostname));
        matching.map(|device| device.identifier.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(json: &str) -> DeviceList {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn a_key_request_is_single_use_preauthorized_non_ephemeral_and_expires() {
        let tags: TailnetTags = std::iter::once(TailnetTag::new("tag:app").unwrap()).collect();
        let body = serde_json::to_value(KeyRequest::single_use(&tags)).unwrap();
        let expected = serde_json::json!({
            "capabilities": { "devices": { "create": {
                "reusable": false, "ephemeral": false, "preauthorized": true,
                "tags": ["tag:app"],
            }}},
            "expirySeconds": 3600,
        });
        assert_eq!(body, expected);
    }

    #[test]
    fn hostnames_match_ignoring_case() {
        let devices = list(r#"{"devices":[{"id":"1","hostname":"App","tags":["tag:a"]}]}"#);
        assert_eq!(devices.ids_of("aPP"), ["1"]);
    }

    #[test]
    fn untagged_and_nameless_devices_never_match() {
        let devices = list(
            r#"{"devices":[
                {"id":"1","hostname":"app"},
                {"id":"2","hostname":"app","tags":[]},
                {"id":"3","tags":["tag:a"]}
            ]}"#,
        );
        assert!(devices.ids_of("app").is_empty());
    }

    #[test]
    fn aegis_nodes_never_match() {
        let devices = list(
            r#"{"devices":[
                {"id":"1","hostname":"app","tags":["tag:aegis-agent"]},
                {"id":"2","hostname":"app","tags":["tag:app","tag:aegis-controller"]}
            ]}"#,
        );
        assert!(devices.ids_of("app").is_empty());
    }

    #[test]
    fn an_empty_list_matches_nothing() {
        assert!(list(r#"{"devices":[]}"#).ids_of("app").is_empty());
    }
}
