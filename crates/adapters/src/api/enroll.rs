//! `POST /v1/enroll`: the controller's side, which answers an agent's enrolment request, and the
//! client an agent uses to ask it.

use std::sync::Arc;

use aegis_api::version_1::{ENROLL_PATH, EnrollRequest, EnrollResponse, SidecarVolume};
use aegis_application::{
    Caller, EnrollError, Enrolled, Enroller, EnrollmentRequest, Grant, SidecarEnroller,
    SidecarIdentity, StateVolume, TagError,
};
use aegis_domain::{NodeLabel, StackName, TailnetTag, TailnetTags, ValueError};
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Extension, Json, Router};

use super::refusal::Refusal;
use super::server::ServeError;
use super::{Binding, ClientError};
use crate::tailscale::{ClientId, ClientSecret, OAuthCredentials, StackNodes, TailscaleApi};

/// What the enrolment handler works with: who answers, and how a request is bound to its caller.
struct Gated<Enrol> {
    enroller: Enrol,
    binding: Binding,
}

/// The `POST /v1/enroll` route: answer an agent's request with `enroller`.
///
/// `binding` first checks the request against the caller's host and the desired state. The route
/// is meant to be served behind the tag check of the controller.
pub fn enroll_routes<Enrol>(enroller: Enrol, binding: Binding) -> Router
where
    Enrol: SidecarEnroller + Send + Sync + 'static,
{
    Router::new()
        .route(ENROLL_PATH, post(enroll::<Enrol>))
        .with_state(Arc::new(Gated { enroller, binding }))
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
    let stack = StackName::new(request.stack())?;
    let tags = tags_of(request.tags())?;
    let identity = SidecarIdentity::new(stack, node);
    Ok(EnrollmentRequest::new(
        identity,
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

async fn enroll<Enrol>(
    State(gated): State<Arc<Gated<Enrol>>>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<EnrollRequest>,
) -> Result<Json<EnrollResponse>, Refusal>
where
    Enrol: SidecarEnroller + Send + Sync + 'static,
{
    let host = gated.binding.host_of(&caller)?;
    let request =
        request_of(&request).map_err(|error| Refusal::new(StatusCode::BAD_REQUEST, &error))?;
    gated.binding.admit_enrollment(&host, &request)?;
    let grant = gated
        .enroller
        .enroll(&request)
        .await
        .map_err(|error| Refusal::new(status_of(&error), &error))?;
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
}

fn reason_of(error: &ClientError) -> aegis_application::Reason {
    match error {
        ClientError::Rejected(status) => aegis_application::Reason::new(status),
        ClientError::Transport(_) | ClientError::TooLarge | ClientError::Undecodable => {
            aegis_application::Reason::new(&"the call failed")
        }
    }
}

fn controller_failure(error: &ClientError) -> EnrollError {
    EnrollError::Controller(reason_of(error))
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
        let node = request.node().as_str();
        let wire = EnrollRequest::new(node, tags, wire_volume(request.state()))
            .for_stack(request.stack().as_str());
        let answer = self.client.enroll(&wire).await;
        grant_of(&answer.map_err(|error| controller_failure(&error))?)
    }
}

impl aegis_application::NodeDeleter for ControllerDesk {
    async fn delete_node(&self, node: &NodeLabel) -> Result<(), aegis_application::Reason> {
        let deleted = self.client.delete_node(node).await;
        deleted.map_err(|error| reason_of(&error))
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

/// The Tailscale side of a controller: what mints the keys and what deletes the nodes, configured
/// from its environment, and not yet bound to a [`Binding`].
#[derive(Debug)]
pub(super) struct EnrollDesks {
    enroller: Enroller<TailscaleApi, TailscaleApi>,
    removing: StackNodes,
}

impl EnrollDesks {
    /// The `POST /v1/enroll` and `DELETE /v1/nodes/{hostname}` routes, which check each request
    /// with `binding`.
    pub(super) fn routes(self, binding: &Binding) -> Router {
        let enroll = enroll_routes(self.enroller, binding.clone());
        enroll.merge(super::node_routes(self.removing, binding.clone()))
    }
}

/// The Tailscale OAuth client is in `AEGIS_TS_CLIENT_ID` and `AEGIS_TS_CLIENT_SECRET` (decrypted
/// by the deployment that runs the controller), and the tags a stack's node may carry are in
/// `AEGIS_ENROLL_TAGS`.
///
/// # Errors
///
/// Returns a [`ServeError`] when a variable is missing or invalid, or the Tailscale client cannot
/// be built.
pub(super) fn enroll_desks_from_env() -> Result<EnrollDesks, ServeError> {
    let credentials = OAuthCredentials::new(
        ClientId::new(required(CLIENT_ID_VAR)?),
        ClientSecret::new(required(CLIENT_SECRET_VAR)?),
    );
    let allowed = allowed_tags(&required(TAGS_VAR)?)?;
    let api = TailscaleApi::new(credentials).map_err(|error| ServeError::Invalid {
        variable: CLIENT_ID_VAR,
        cause: error.to_string(),
    })?;
    let removing = api.clone().for_stack_nodes(allowed.clone());
    let enroller = Enroller::new(api.clone(), api, allowed);
    Ok(EnrollDesks { enroller, removing })
}
