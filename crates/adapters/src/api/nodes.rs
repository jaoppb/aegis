//! `DELETE /v1/nodes/{hostname}`: the controller's side, which removes the tailnet node of a
//! purged stack, and the agent's [`NodeDeleter`] that asks for it. The agent holds no Tailscale
//! credentials: the controller does the call.

use std::sync::Arc;

use aegis_api::version_1::{NODE_PATH, node_path};
use aegis_application::{DeviceRef, NodeDeleter, NodeRemover, Reason, TailnetError};
use aegis_domain::{HostName, NodeLabel};
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::delete;
use reqwest::StatusCode as ClientStatus;

use super::{ApiClient, ClientError, ControllerDesk};

/// The `DELETE /v1/nodes/{hostname}` route: delete the node with `remover`. It is meant to be
/// served behind the tag check of the controller.
pub fn node_routes<Remover>(remover: Remover) -> Router
where
    Remover: NodeRemover + Send + Sync + 'static,
{
    Router::new()
        .route(NODE_PATH, delete(remove::<Remover>))
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

async fn remove<Remover>(
    State(remover): State<Arc<Remover>>,
    Path(hostname): Path<String>,
) -> Result<StatusCode, Refusal>
where
    Remover: NodeRemover + Send + Sync + 'static,
{
    let device = device_of(&hostname)?;
    let outcome = remover.remove(&device).await;
    outcome
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| Refusal {
            status: status_of(&error),
            reason: error.to_string(),
        })
}

impl ApiClient {
    /// Ask the controller to delete the tailnet node `hostname`.
    ///
    /// # Errors
    ///
    /// Returns [`ClientError::Rejected`] when the controller refuses, including `404` for a node
    /// that does not exist, and [`ClientError::Transport`] when the call fails.
    pub async fn delete_node(&self, hostname: &NodeLabel) -> Result<(), ClientError> {
        let url = self.base().join(&node_path(hostname.as_str()));
        let response = self.http().delete(url).send().await?;
        let status = response.status();
        status
            .is_success()
            .then_some(())
            .ok_or(ClientError::Rejected(status))
    }
}

impl NodeDeleter for ControllerDesk {
    async fn delete_node(&self, node: &NodeLabel) -> Result<(), Reason> {
        match self.client().delete_node(node).await {
            Ok(()) | Err(ClientError::Rejected(ClientStatus::NOT_FOUND)) => Ok(()),
            Err(ClientError::Rejected(status)) => Err(Reason::new(&status)),
            Err(ClientError::Transport(_)) => Err(Reason::new(&"the call failed")),
        }
    }
}
