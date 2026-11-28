//! The Tailscale HTTP API, held only by the controller: the OAuth client-credentials exchange,
//! single-use tagged auth keys and device deletion. The Tailscale CLI cannot do any of this.

mod client;
mod credentials;
mod wire;

pub use client::TailscaleApi;
pub use credentials::{ClientId, ClientSecret, OAuthCredentials};
