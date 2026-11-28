//! The reqwest client of the Tailscale API.

use aegis_application::{
    AuthKey, DeviceId, DeviceRef, KeyMinter, NodeRemover, Reason, TailnetError, TailnetNames,
};
use aegis_domain::{HostName, TailnetName, TailnetTags};
use reqwest::{RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;

use super::credentials::OAuthCredentials;
use super::wire::{DeviceList, KeyReply, KeyRequest, TokenReply};
use crate::http::BaseUrl;

/// The public Tailscale API.
const OFFICIAL_API: &str = "https://api.tailscale.com";

const TOKEN_PATH: &str = "/api/v2/oauth/token";
const KEYS_PATH: &str = "/api/v2/tailnet/-/keys";
const DEVICES_PATH: &str = "/api/v2/tailnet/-/devices";

/// The most an answer may weigh. The largest one is the device list of a homelab tailnet.
const MAX_ANSWER_BYTES: usize = 1 << 20;

/// What a token may do, so a stolen token is worth no more than its operation needs.
#[derive(Debug, Clone, Copy)]
enum Scope {
    /// Create auth keys.
    AuthKeys,
    /// List and delete devices.
    Devices,
}

impl Scope {
    const fn name(self) -> &'static str {
        match self {
            Self::AuthKeys => "auth_keys",
            Self::Devices => "devices:core",
        }
    }
}

/// Calls the Tailscale API as an OAuth client. Every operation exchanges the credentials for a
/// fresh access token first, and nothing it prints or returns in an error holds a secret.
#[derive(Debug, Clone)]
pub struct TailscaleApi {
    http: reqwest::Client,
    base: BaseUrl,
    credentials: OAuthCredentials,
}

impl TailscaleApi {
    /// A client of the public Tailscale API.
    ///
    /// # Errors
    ///
    /// Returns a [`TailnetError`] when the HTTP client cannot be built.
    pub fn new(credentials: OAuthCredentials) -> Result<Self, TailnetError> {
        Self::with_base(BaseUrl::new(OFFICIAL_API), credentials)
    }

    /// A client of the API served at `base`, which receives the credentials. Only an `https`
    /// URL or a loopback address (a fake server in a test) is accepted.
    ///
    /// # Errors
    ///
    /// Returns [`TailnetError::InsecureBase`] for any other URL, and another [`TailnetError`]
    /// when the HTTP client cannot be built.
    pub fn with_base(base: BaseUrl, credentials: OAuthCredentials) -> Result<Self, TailnetError> {
        let protected = base.is_protected();
        protected.then_some(()).ok_or(TailnetError::InsecureBase)?;
        let http = BaseUrl::client().map_err(|failure| unavailable(&failure.without_url()))?;
        Ok(Self {
            http,
            base,
            credentials,
        })
    }

    async fn token(&self, scope: Scope) -> Result<String, TailnetError> {
        let form = [
            ("grant_type", "client_credentials"),
            ("client_id", self.credentials.client().as_str()),
            ("client_secret", self.credentials.secret().expose()),
            ("scope", scope.name()),
        ];
        let request = self.http.post(self.base.join(TOKEN_PATH)).form(&form);
        let reply: TokenReply = fetch(request).await?;
        Ok(reply.into_token())
    }

    async fn resolve(&self, node: &DeviceRef, token: &str) -> Result<DeviceId, TailnetError> {
        match node {
            DeviceRef::Identifier(device) => Ok(device.clone()),
            DeviceRef::Hostname(hostname) => self.find(hostname, token).await,
        }
    }

    async fn find(&self, hostname: &HostName, token: &str) -> Result<DeviceId, TailnetError> {
        let url = self.base.join(DEVICES_PATH);
        let list: DeviceList = fetch(self.http.get(url).bearer_auth(token)).await?;
        let ids = list.ids_of(hostname.as_str());
        match ids.as_slice() {
            [only] => DeviceId::new(*only).map_err(|failure| unavailable(&failure)),
            [] => Err(TailnetError::NoSuchDevice),
            _ => Err(TailnetError::AmbiguousHostname),
        }
    }
}

impl KeyMinter for TailscaleApi {
    async fn mint(&self, tags: &TailnetTags) -> Result<AuthKey, TailnetError> {
        let token = self.token(Scope::AuthKeys).await?;
        let url = self.base.join(KEYS_PATH);
        let body = KeyRequest::single_use(tags);
        let request = self.http.post(url).bearer_auth(token).json(&body);
        let reply: KeyReply = fetch(request).await?;
        Ok(AuthKey::new(reply.into_key()))
    }
}

impl TailnetNames for TailscaleApi {
    async fn tailnet(&self) -> Result<TailnetName, TailnetError> {
        let token = self.token(Scope::Devices).await?;
        let url = self.base.join(DEVICES_PATH);
        let list: DeviceList = fetch(self.http.get(url).bearer_auth(token)).await?;
        list.tailnet()
            .ok_or_else(|| unavailable(&"no device names the tailnet"))
    }
}

impl NodeRemover for TailscaleApi {
    async fn remove(&self, node: &DeviceRef) -> Result<(), TailnetError> {
        let token = self.token(Scope::Devices).await?;
        let device = self.resolve(node, &token).await?;
        let url = self.base.join(&format!("/api/v2/device/{device}"));
        delete(self.http.delete(url).bearer_auth(token)).await
    }
}

/// A failed call, described without the request or the answer: neither may hold a secret.
fn unavailable(cause: &impl std::fmt::Display) -> TailnetError {
    TailnetError::Unavailable(Reason::new(cause))
}

async fn transmit(request: RequestBuilder) -> Result<Response, TailnetError> {
    let sent = request.send().await;
    sent.map_err(|failure| unavailable(&failure.without_url()))
}

/// Accept only a success answer.
fn accept(response: Response) -> Result<Response, TailnetError> {
    let status = response.status();
    status
        .is_success()
        .then_some(response)
        .ok_or_else(|| TailnetError::Rejected(Reason::new(&status)))
}

/// Send `request` and read a success answer as `Reply`.
async fn fetch<Reply: DeserializeOwned>(request: RequestBuilder) -> Result<Reply, TailnetError> {
    read(accept(transmit(request).await?)?).await
}

/// Delete a device. A `404` means it is already gone.
async fn delete(request: RequestBuilder) -> Result<(), TailnetError> {
    let response = transmit(request).await?;
    match response.status() {
        StatusCode::NOT_FOUND => Err(TailnetError::NoSuchDevice),
        _ => accept(response).map(drop),
    }
}

fn append(body: &mut Vec<u8>, chunk: &[u8]) -> Result<(), TailnetError> {
    body.extend_from_slice(chunk);
    let fits = body.len() <= MAX_ANSWER_BYTES;
    fits.then_some(())
        .ok_or_else(|| unavailable(&"the answer is too large"))
}

/// Read the answer as `Reply`, refusing one that is too large. The failure text is fixed, so an
/// answer that echoes a secret back cannot put it in an error.
async fn read<Reply: DeserializeOwned>(mut answer: Response) -> Result<Reply, TailnetError> {
    let mut body = Vec::new();
    while let Some(chunk) = answer.chunk().await.map_err(|_| unreadable())? {
        append(&mut body, &chunk)?;
    }
    serde_json::from_slice(&body).map_err(|_| unreadable())
}

fn unreadable() -> TailnetError {
    unavailable(&"the answer could not be read")
}
