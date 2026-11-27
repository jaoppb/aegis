//! The [`StackParser`] over `serde_norway` (compose) and `toml` (policy).

use aegis_application::{ParseError, StackParser};
use aegis_domain::{ComposeSummary, StackPolicy};

use crate::{compose, policy};

/// Parses `compose.yaml` and `aegis.toml` files.
#[derive(Debug, Clone, Copy, Default)]
pub struct SerdeStackParser;

impl StackParser for SerdeStackParser {
    fn parse_compose(&self, text: &str) -> Result<ComposeSummary, ParseError> {
        compose::parse(text)
    }

    fn parse_policy(&self, text: &str) -> Result<StackPolicy, ParseError> {
        policy::parse(text)
    }
}
