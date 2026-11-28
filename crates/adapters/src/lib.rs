//! Aegis adapters layer: the `std::fs` repository reader and the YAML and TOML parsers that
//! implement the application ports.

pub mod api;
pub mod compose;
mod filesystem;
pub(crate) mod http;
mod parser;
pub mod policy;
mod sops;
pub mod tailscale;
mod whois;

pub use filesystem::FsRepo;
pub use http::BaseUrl;
pub use parser::SerdeStackParser;
pub use sops::SopsCheck;
pub use whois::TailscaleWhois;
