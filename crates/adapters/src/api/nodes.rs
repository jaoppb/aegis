//! `DELETE /v1/nodes/{hostname}`: the controller's side, which removes the tailnet node of a
//! purged stack, and the agent's [`NodeDeleter`] that asks for it. The agent holds no Tailscale
//! credentials: the controller does the call.

use std::sync::Arc;

use aegis_api::version_1::{NODE_GONE, NODE_PATH};
use aegis_application::{DeviceRef, NodeRemover, TailnetError};

use crate::tailscale::StackNodes;
use aegis_domain::{HostName, NodeLabel};
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::delete;

/// The `DELETE /v1/nodes/{hostname}` route: delete the stack node with `remover`, which refuses
/// any device a stack may not own. It is meant to be
/// served behind the tag check of the controller.
pub fn node_routes(remover: StackNodes) -> Router {
    Router::new()
        .route(NODE_PATH, delete(remove))
        .with_state(Arc::new(remover))
}

/// A refused deletion: a status and a short plain-text reason that holds no secrets.
struct Refusal {
    status: StatusCode,
    reason: String,
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        (self.status, self.reason).into_response()
    }
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

fn device_of(hostname: &str) -> Result<DeviceRef, Refusal> {
    let invalid = |error: aegis_domain::ValueError| Refusal {
        status: StatusCode::BAD_REQUEST,
        reason: error.to_string(),
    };
    let label = NodeLabel::new(hostname).map_err(invalid)?;
    let name = HostName::new(label.as_str()).map_err(invalid)?;
    Ok(DeviceRef::Hostname(name))
}

async fn remove(
    State(remover): State<Arc<StackNodes>>,
    Path(hostname): Path<String>,
) -> Result<StatusCode, Refusal> {
    let device = device_of(&hostname)?;
    let outcome = remover.remove(&device).await;
    outcome
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| Refusal {
            status: status_of(&error),
            reason: reason_of(&error),
        })
}
