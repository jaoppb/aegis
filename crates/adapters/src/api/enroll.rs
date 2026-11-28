//! `POST /v1/enroll`: the controller's side, which answers an agent's enrolment request, and the
//! client an agent uses to ask it.

use std::sync::Arc;

use aegis_api::version_1::{ENROLL_PATH, EnrollRequest, EnrollResponse, SidecarVolume};
use aegis_application::{
    EnrollError, Enrolled, Enroller, EnrollmentRequest, Grant, SidecarEnroller, StateVolume,
    TagError,
};
use aegis_domain::{NodeLabel, TailnetTag, TailnetTags, ValueError};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};

use super::ClientError;
use super::server::ServeError;
use crate::tailscale::{ClientId, ClientSecret, OAuthCredentials, TailscaleApi};

/// The `POST /v1/enroll` route: answer an agent's request with `enroller`. It is meant to be
/// served behind the tag check of the controller.
pub fn enroll_routes<Desk>(enroller: Desk) -> Router
where
    Desk: SidecarEnroller + Send + Sync + 'static,
{
    Router::new()
        .route(ENROLL_PATH, post(enroll::<Desk>))
        .with_state(Arc::new(enroller))
}

/// A refused enrolment: a status and a short plain-text reason that holds no secrets.
struct Refusal {
    status: StatusCode,
    reason: String,
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        (self.status, self.reason).into_response()
    }
}

fn refuse(status: StatusCode, reason: &impl std::fmt::Display) -> Refusal {
    Refusal {
        status,
        reason: reason.to_string(),
    }
}

const fn status_of(error: &EnrollError) -> StatusCode {
    match error {
        EnrollError::Tags(TagError::Empty) => StatusCode::BAD_REQUEST,
        EnrollError::Tags(_) => StatusCode::FORBIDDEN,
        EnrollError::Mint(_) | EnrollError::Tailnet(_) | EnrollError::Controller(_) => {
            StatusCode::BAD_GATEWAY
        }
    }
}

const fn volume_of(volume: SidecarVolume) -> StateVolume {
    match volume {
        SidecarVolume::Empty => StateVolume::Empty,
        SidecarVolume::Populated => StateVolume::Populated,
    }
}

const fn wire_volume(volume: StateVolume) -> SidecarVolume {
    match volume {
        StateVolume::Empty => SidecarVolume::Empty,
        StateVolume::Populated => SidecarVolume::Populated,
    }
}

fn tags_of(names: &[String]) -> Result<TailnetTags, ValueError> {
    let tags = names.iter().map(TailnetTag::new);
    tags.collect::<Result<Vec<_>, _>>().map(TailnetTags::new)
}

fn request_of(request: &EnrollRequest) -> Result<EnrollmentRequest, ValueError> {
    let node = NodeLabel::new(request.hostname())?;
    let tags = tags_of(request.tags())?;
    Ok(EnrollmentRequest::new(
        node,
        tags,
        volume_of(request.state()),
    ))
}

fn response_of(grant: &Grant) -> EnrollResponse {
    let key = match grant.enrolled() {
        Enrolled::Minted(key) => Some(key.expose().to_owned()),
        Enrolled::AlreadyEnrolled => None,
    };
    EnrollResponse::new(grant.tailnet().as_str(), key)
}

async fn enroll<Desk>(
    State(desk): State<Arc<Desk>>,
    Json(request): Json<EnrollRequest>,
) -> Result<Json<EnrollResponse>, Refusal>
where
    Desk: SidecarEnroller + Send + Sync + 'static,
{
    let request = request_of(&request).map_err(|error| refuse(StatusCode::BAD_REQUEST, &error))?;
    let grant = desk
        .enroll(&request)
        .await
        .map_err(|error| refuse(status_of(&error), &error))?;
    Ok(Json(response_of(&grant)))
}

/// Asks the controller to enrol a node. Authentication is the agent's tailnet identity.
#[derive(Debug, Clone)]
pub struct ControllerDesk {
    client: super::ApiClient,
}

impl ControllerDesk {
    /// A desk that asks the controller `client` calls.
    #[must_use]
    pub const fn new(client: super::ApiClient) -> Self {
        Self { client }
    }

    pub(super) const fn client(&self) -> &super::ApiClient {
        &self.client
    }
}

fn controller_failure(error: &ClientError) -> EnrollError {
    let reason = match error {
        ClientError::Rejected(status) => aegis_application::Reason::new(status),
        ClientError::Transport(_) => aegis_application::Reason::new(&"the call failed"),
    };
    EnrollError::Controller(reason)
}

fn grant_of(response: &EnrollResponse) -> Result<Grant, EnrollError> {
    let tailnet = aegis_domain::TailnetName::new(response.tailnet()).map_err(|_| {
        EnrollError::Controller(aegis_application::Reason::new(
            &"the controller sent an invalid tailnet name",
        ))
    })?;
    let enrolled = response.key().map_or(Enrolled::AlreadyEnrolled, |key| {
        Enrolled::Minted(aegis_application::AuthKey::new(key))
    });
    Ok(Grant::new(tailnet, enrolled))
}

impl SidecarEnroller for ControllerDesk {
    async fn enroll(&self, request: &EnrollmentRequest) -> Result<Grant, EnrollError> {
        let tags = request.tags().iter().map(ToString::to_string).collect();
        let wire = EnrollRequest::new(request.node().as_str(), tags, wire_volume(request.state()));
        let answer = self.client.enroll(&wire).await;
        grant_of(&answer.map_err(|error| controller_failure(&error))?)
    }
}

/// The variable that holds the id of the Tailscale OAuth client.
pub const CLIENT_ID_VAR: &str = "AEGIS_TS_CLIENT_ID";
/// The variable that holds the secret of the Tailscale OAuth client.
pub const CLIENT_SECRET_VAR: &str = "AEGIS_TS_CLIENT_SECRET";
const TAGS_VAR: &str = "AEGIS_ENROLL_TAGS";

/// The variables a controller refuses to start without, besides `AEGIS_LISTEN`.
pub const CONTROLLER_REQUIRED: [&str; 3] = [CLIENT_ID_VAR, CLIENT_SECRET_VAR, TAGS_VAR];

fn required(name: &'static str) -> Result<String, ServeError> {
    std::env::var(name).map_err(|_| ServeError::Missing(name))
}

/// The ACL tags of `AEGIS_ENROLL_TAGS`, comma-separated: the only tags a stack's node may carry.
fn allowed_tags(variable: &str) -> Result<TailnetTags, ServeError> {
    TailnetTags::parse_list(variable).map_err(|error| ServeError::Invalid {
        variable: TAGS_VAR,
        cause: error.to_string(),
    })
}

/// The `POST /v1/enroll` and `DELETE /v1/nodes/{hostname}` routes of a controller, configured from its environment.
///
/// The Tailscale OAuth client is in `AEGIS_TS_CLIENT_ID` and `AEGIS_TS_CLIENT_SECRET` (decrypted
/// by the deployment that runs the controller), and the tags a stack's node may carry are in
/// `AEGIS_ENROLL_TAGS`.
///
/// # Errors
///
/// Returns a [`ServeError`] when a variable is missing or invalid, or the Tailscale client cannot
/// be built.
pub(super) fn enroll_routes_from_env() -> Result<Router, ServeError> {
    let credentials = OAuthCredentials::new(
        ClientId::new(required(CLIENT_ID_VAR)?),
        ClientSecret::new(required(CLIENT_SECRET_VAR)?),
    );
    let allowed = allowed_tags(&required(TAGS_VAR)?)?;
    let api = TailscaleApi::new(credentials).map_err(|error| ServeError::Invalid {
        variable: CLIENT_ID_VAR,
        cause: error.to_string(),
    })?;
    let enroll = enroll_routes(Enroller::new(api.clone(), api.clone(), allowed));
    Ok(enroll.merge(super::node_routes(api)))
}
