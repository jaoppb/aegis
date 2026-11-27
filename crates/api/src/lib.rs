//! The wire types of the Aegis controller and agent HTTP API.
//!
//! Each API version lives in its own module (`version_1` today), so a change that breaks clients gets a
//! new module and the old one keeps serving. The crate holds data shapes only: no IO, no HTTP
//! stack and no dependency on another Aegis crate.

pub mod version_1;
