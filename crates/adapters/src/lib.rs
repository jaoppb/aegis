//! Aegis adapters layer: the `std::fs` repository reader and the YAML and TOML parsers that
//! implement the application ports.

pub mod api;
pub mod compose;
mod filesystem;
mod parser;
pub mod policy;
mod whois;

pub use filesystem::FsRepo;
pub use parser::SerdeStackParser;
pub use whois::TailscaleWhois;
