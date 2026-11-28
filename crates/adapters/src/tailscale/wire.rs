//! The JSON and form bodies of the Tailscale API.

use aegis_application::TailnetError;
use aegis_domain::TailnetTags;
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
    ///
    /// # Errors
    ///
    /// Returns [`TailnetError::NoTags`] when `tags` is empty: Tailscale needs a tag to own the node.
    pub(super) fn single_use(tags: &TailnetTags) -> Result<Self, TailnetError> {
        let tagged = !tags.is_empty();
        tagged.then_some(()).ok_or(TailnetError::NoTags)?;
        let create = Create {
            reusable: Switch::Disabled,
            ephemeral: Switch::Disabled,
            preauthorized: Switch::Enabled,
            tags: tags.iter().map(ToString::to_string).collect(),
        };
        Ok(Self {
            capabilities: Capabilities {
                devices: Devices { create },
            },
            expiry_seconds: KEY_LIFETIME_SECONDS,
        })
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
}

impl Device {
    fn is_named(&self, hostname: &str) -> bool {
        let found = self.hostname.as_deref();
        found.is_some_and(|name| name.eq_ignore_ascii_case(hostname))
    }
}

impl DeviceList {
    /// The ids of the devices whose hostname is `hostname`, ignoring case as DNS does.
    pub(super) fn ids_of(&self, hostname: &str) -> Vec<&str> {
        let matching = self
            .devices
            .iter()
            .filter(|device| device.is_named(hostname));
        matching.map(|device| device.identifier.as_str()).collect()
    }
}
