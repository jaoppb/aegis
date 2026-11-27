//! Reads an `aegis.toml`, rejecting every key outside the schema.

use aegis_application::ParseError;
use aegis_domain::{
    Expose, Port, ServiceName, StackPolicy, StackState, TailnetTag, UpdatePolicy, ValueError,
};
use serde::Deserialize;
use toml::{Table, Value};

const TOP_KEYS: [&str; 4] = ["expose", "update", "tailnet_tags", "state"];
const EXPOSE_KEYS: [&str; 2] = ["service", "port"];

#[derive(Deserialize)]
struct RawPolicy {
    expose: Option<RawExpose>,
    update: Option<RawUpdate>,
    tailnet_tags: Option<Vec<String>>,
    state: Option<RawState>,
}

#[derive(Deserialize)]
struct RawExpose {
    service: String,
    port: u16,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum RawUpdate {
    Manual,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum RawState {
    Active,
    Purged,
}

/// Read `text` as a policy. Unknown keys, at the top level or under `expose`, are an error.
///
/// # Errors
///
/// Returns [`ParseError::UnknownKey`] for a key outside the schema and [`ParseError::Invalid`] for
/// malformed TOML or an invalid value.
pub fn parse(text: &str) -> Result<StackPolicy, ParseError> {
    let table: Table = text
        .parse()
        .map_err(|error: toml::de::Error| invalid(&error.message()))?;
    if let Some(key) = unknown_key(&table) {
        return Err(ParseError::UnknownKey(key));
    }
    let raw = table
        .try_into()
        .map_err(|error: toml::de::Error| invalid(&error.message()))?;
    policy(raw).map_err(|error| invalid(&error))
}

fn invalid(reason: &impl ToString) -> ParseError {
    ParseError::Invalid(reason.to_string())
}

fn unknown_in(table: &Table, allowed: &[&str], prefix: &str) -> Option<String> {
    let unknown = table.keys().find(|key| !allowed.contains(&key.as_str()));
    unknown.map(|key| format!("{prefix}{key}"))
}

fn unknown_key(table: &Table) -> Option<String> {
    let expose = table.get("expose").and_then(Value::as_table);
    unknown_in(table, &TOP_KEYS, "")
        .or_else(|| expose.and_then(|inner| unknown_in(inner, &EXPOSE_KEYS, "expose.")))
}

fn policy(raw: RawPolicy) -> Result<StackPolicy, ValueError> {
    let tags = raw.tailnet_tags.unwrap_or_default().into_iter();
    Ok(StackPolicy {
        expose: raw.expose.map(expose).transpose()?,
        update: raw
            .update
            .map_or(UpdatePolicy::Manual, |RawUpdate::Manual| {
                UpdatePolicy::Manual
            }),
        tailnet_tags: tags.map(TailnetTag::new).collect::<Result<_, _>>()?,
        state: raw.state.map_or(StackState::Active, state),
    })
}

fn expose(raw: RawExpose) -> Result<Expose, ValueError> {
    Ok(Expose {
        service: ServiceName::new(raw.service)?,
        port: Port::new(raw.port)?,
    })
}

const fn state(raw: RawState) -> StackState {
    match raw {
        RawState::Active => StackState::Active,
        RawState::Purged => StackState::Purged,
    }
}
