//! Aegis adapters layer: the `std::fs` repository reader and the YAML and TOML parsers that
//! implement the application ports.

pub mod allowlist;
pub mod api;
pub mod cli;
pub mod compose;
mod decrypting;
mod docker_compose;
mod docker_state;
mod filesystem;
mod git;
pub(crate) mod http;
mod parser;
pub mod policy;
mod sidecar_compose;
pub mod sops;
mod sops_decrypt;
pub mod tailscale;
mod tmpfs_store;
mod whois;

pub use decrypting::{DecryptingCompose, SecretPorts};
pub use docker_compose::DockerCompose;
pub use docker_state::DockerStateProbe;
pub use filesystem::FsRepo;
pub use git::GitCheckout;
pub use http::BaseUrl;
pub use parser::SerdeStackParser;
pub use sidecar_compose::{SidecarCompose, SidecarPorts};
pub use sops::SopsCheck;
pub use sops_decrypt::SopsDecrypt;
pub use tmpfs_store::TmpfsStore;
pub use whois::TailscaleWhois;
