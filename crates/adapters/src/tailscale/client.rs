//! The reqwest client of the Tailscale API.

use std::time::Duration;

use aegis_application::{
    AuthKey, DeviceId, DeviceRef, KeyMinter, NodeRemover, Reason, TailnetError,
};
use aegis_domain::{HostName, TailnetTags};
use reqwest::{RequestBuilder, Response, StatusCode};

use super::credentials::OAuthCredentials;
use super::wire::{DeviceList, KeyReply, KeyRequest, TokenReply};
use crate::api::BaseUrl;

/// How long a call may take before it fails.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The public Tailscale API.
const OFFICIAL_API: &str = "https://api.tailscale.com";

const TOKEN_PATH: &str = "/api/v2/oauth/token";
const KEYS_PATH: &str = "/api/v2/tailnet/-/keys";
const DEVICES_PATH: &str = "/api/v2/tailnet/-/devices";

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

    /// A client of the API served at `base`. The credentials are sent to `base`, so use anything
    /// but the public API only for a fake server in a test.
    ///
    /// # Errors
    ///
    /// Returns a [`TailnetError`] when the HTTP client cannot be built.
    pub fn with_base(base: BaseUrl, credentials: OAuthCredentials) -> Result<Self, TailnetError> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|failure| unavailable(&failure.without_url()))?;
        Ok(Self {
            http,
            base,
            credentials,
        })
    }

    async fn token(&self) -> Result<String, TailnetError> {
        let form = [
            ("grant_type", "client_credentials"),
            ("client_id", self.credentials.client().as_str()),
            ("client_secret", self.credentials.secret().expose()),
        ];
        let request = self.http.post(self.base.join(TOKEN_PATH)).form(&form);
        let reply: TokenReply = read(send(request).await?).await?;
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
        let list: DeviceList = read(send(self.http.get(url).bearer_auth(token)).await?).await?;
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
        let body = KeyRequest::single_use(tags)?;
        let token = self.token().await?;
        let url = self.base.join(KEYS_PATH);
        let request = self.http.post(url).bearer_auth(token).json(&body);
        let reply: KeyReply = read(send(request).await?).await?;
        Ok(AuthKey::new(reply.into_key()))
    }
}

impl NodeRemover for TailscaleApi {
    async fn remove(&self, node: &DeviceRef) -> Result<(), TailnetError> {
        let token = self.token().await?;
        let device = self.resolve(node, &token).await?;
        let url = self.base.join(&format!("/api/v2/device/{device}"));
        send(self.http.delete(url).bearer_auth(token)).await?;
        Ok(())
    }
}

/// A failed call, described without the request or the response body: neither may hold a secret.
fn unavailable(cause: &impl std::fmt::Display) -> TailnetError {
    TailnetError::Unavailable(Reason::new(cause))
}

/// Send `request` and accept only a success answer. A `404` means the device is gone.
async fn send(request: RequestBuilder) -> Result<Response, TailnetError> {
    let response = request
        .send()
        .await
        .map_err(|failure| unavailable(&failure.without_url()))?;
    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Err(TailnetError::NoSuchDevice);
    }
    status
        .is_success()
        .then_some(response)
        .ok_or_else(|| TailnetError::Rejected(Reason::new(&status)))
}

async fn read<Reply: serde::de::DeserializeOwned>(
    response: Response,
) -> Result<Reply, TailnetError> {
    response
        .json()
        .await
        .map_err(|failure| unavailable(&failure.without_url()))
}
