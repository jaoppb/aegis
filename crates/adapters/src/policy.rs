//! Reads an `aegis.toml`, rejecting every key outside the schema.

use aegis_application::ParseError;
use aegis_domain::{
    Expose, Line, Node, Port, ServiceName, StackPolicy, StackState, TailnetTag, TailnetTags,
    UpdatePolicy, ValueError,
};
use serde::Deserialize;
use toml::{Table, Value};

const TOP_KEYS: [&str; 5] = ["expose", "node", "update", "tailnet_tags", "state"];
const EXPOSE_KEYS: [&str; 2] = ["service", "port"];
const NODE_KEYS: [&str; 1] = ["service"];

#[derive(Deserialize)]
struct RawPolicy {
    #[serde(flatten)]
    reach: RawReach,
    update: Option<RawUpdate>,
    tailnet_tags: Option<Vec<String>>,
    state: Option<RawState>,
}

#[derive(Deserialize)]
struct RawReach {
    expose: Option<RawExpose>,
    node: Option<RawNode>,
}

#[derive(Deserialize)]
struct RawExpose {
    service: String,
    port: u16,
}

#[derive(Deserialize)]
struct RawNode {
    service: String,
}

#[derive(Deserialize, Clone, Copy)]
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

/// Read `text` as a policy. Unknown keys, at the top level or under `expose` or `node`, are an error.
///
/// # Errors
///
/// Returns [`ParseError::UnknownKey`] for a key outside the schema and [`ParseError::Invalid`] for
/// malformed TOML or an invalid value.
pub fn parse(text: &str) -> Result<StackPolicy, ParseError> {
    let table: Table = text
        .parse()
        .map_err(|error: toml::de::Error| located(text, &error))?;
    if let Some(key) = unknown_key(&table) {
        return Err(ParseError::UnknownKey(key));
    }
    if table.contains_key("expose") && table.contains_key("node") {
        return Err(invalid(
            &"`expose` and `node` exclude each other: a stack has one tailnet node",
        ));
    }
    let raw = table
        .try_into()
        .map_err(|error: toml::de::Error| invalid(&error.message()))?;
    policy(raw).map_err(|error| invalid(&error))
}

/// A TOML error at the line of its span, when it has one.
fn located(text: &str, error: &toml::de::Error) -> ParseError {
    let reason = error.message().to_owned();
    let before = error.span().and_then(|span| text.get(..span.start));
    let line = before.and_then(|head| Line::new(head.matches('\n').count().saturating_add(1)));
    match line {
        Some(line) => ParseError::InvalidAt { reason, line },
        None => ParseError::Invalid(reason),
    }
}

fn invalid(reason: &impl ToString) -> ParseError {
    ParseError::Invalid(reason.to_string())
}

fn unknown_in(table: &Table, allowed: &[&str], prefix: &str) -> Option<String> {
    let unknown = table.keys().find(|key| !allowed.contains(&key.as_str()));
    unknown.map(|key| format!("{prefix}{key}"))
}

fn unknown_key(table: &Table) -> Option<String> {
    let nested = |name: &str, allowed: &[&str]| {
        let inner = table.get(name).and_then(Value::as_table);
        inner.and_then(|found| unknown_in(found, allowed, &format!("{name}.")))
    };
    unknown_in(table, &TOP_KEYS, "")
        .or_else(|| nested("expose", &EXPOSE_KEYS))
        .or_else(|| nested("node", &NODE_KEYS))
}

fn policy(raw: RawPolicy) -> Result<StackPolicy, ValueError> {
    let tags = raw.tailnet_tags.unwrap_or_default().into_iter();
    let update = raw.update.map_or(UpdatePolicy::Manual, update);
    let tags = tags
        .map(TailnetTag::new)
        .collect::<Result<TailnetTags, _>>()?;
    let state = raw.state.map_or(StackState::Active, state);
    let policy = StackPolicy::new(raw.reach.expose.map(expose).transpose()?, tags, state);
    let policy = match raw.reach.node {
        Some(node) => policy.with_node(Node::new(ServiceName::new(node.service)?)),
        None => policy,
    };
    Ok(policy.with_update(update))
}

fn expose(raw: RawExpose) -> Result<Expose, ValueError> {
    Ok(Expose::new(
        ServiceName::new(raw.service)?,
        Port::new(raw.port)?,
    ))
}

const fn update(raw: RawUpdate) -> UpdatePolicy {
    match raw {
        RawUpdate::Manual => UpdatePolicy::Manual,
    }
}

const fn state(raw: RawState) -> StackState {
    match raw {
        RawState::Active => StackState::Active,
        RawState::Purged => StackState::Purged,
    }
}
