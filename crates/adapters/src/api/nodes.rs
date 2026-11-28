//! `DELETE /v1/nodes/{hostname}`: the controller's side, which removes the tailnet node of a
//! purged stack, and the agent's [`NodeDeleter`] that asks for it. The agent holds no Tailscale
//! credentials: the controller does the call.

use std::sync::Arc;

use aegis_api::version_1::{NODE_GONE, NODE_PATH};
use aegis_application::{Caller, TailnetError};
use aegis_domain::{HostName, NodeLabel};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::delete;
use axum::{Extension, Router};

use super::Binding;
use super::refusal::Refusal;
use crate::tailscale::StackNodes;

/// What the deletion handler works with: what deletes the node, and how a request is bound to its
/// caller.
struct Removal {
    remover: StackNodes,
    binding: Binding,
}

/// The `DELETE /v1/nodes/{hostname}` route: delete the stack node with `remover`.
///
/// `remover` refuses any device a stack may not own, and `binding` first checks that the desired
/// state marks a stack of the caller's host purged for that node and no other or rejected stack
/// still holds the name. The route is meant to be served behind the tag check of the controller.
pub fn node_routes(remover: StackNodes, binding: Binding) -> Router {
    Router::new()
        .route(NODE_PATH, delete(remove))
        .with_state(Arc::new(Removal { remover, binding }))
}

fn reason_of(error: &TailnetError) -> String {
    match error {
        TailnetError::NoSuchDevice => NODE_GONE.to_owned(),
        other => other.to_string(),
    }
}

const fn status_of(error: &TailnetError) -> StatusCode {
    match error {
        TailnetError::NoSuchDevice => StatusCode::NOT_FOUND,
        TailnetError::AmbiguousHostname => StatusCode::CONFLICT,
        TailnetError::InsecureBase | TailnetError::Rejected(_) | TailnetError::Unavailable(_) => {
            StatusCode::BAD_GATEWAY
        }
    }
}

fn label_of(hostname: &str) -> Result<NodeLabel, Refusal> {
    NodeLabel::new(hostname).map_err(|error| Refusal::new(StatusCode::BAD_REQUEST, &error))
}

fn device_of(label: &NodeLabel) -> Result<HostName, Refusal> {
    HostName::new(label.as_str()).map_err(|error| Refusal::new(StatusCode::BAD_REQUEST, &error))
}

async fn remove(
    State(removal): State<Arc<Removal>>,
    Extension(caller): Extension<Caller>,
    Path(hostname): Path<String>,
) -> Result<StatusCode, Refusal> {
    let host = removal.binding.host_of(&caller)?;
    let label = label_of(&hostname)?;
    removal.binding.admit_deletion(&host, &label)?;
    let device = device_of(&label)?;
    let outcome = removal.remover.remove(&device).await;
    outcome
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| Refusal::new(status_of(&error), &reason_of(&error)))
}
