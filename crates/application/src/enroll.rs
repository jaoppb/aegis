//! Enrolling the node of a sidecar: what an agent asks the controller, what the controller
//! answers, and the controller's use case that decides.

use std::future::Future;
use std::sync::OnceLock;

use aegis_domain::{HostName, NodeLabel, ProjectName, TailnetName, TailnetTag, TailnetTags};

use crate::source::Reason;
use crate::tailnet::{
    EnrollError, Enrolled, Enrollment, KeyMinter, SidecarState, StateVolume, TagError, TailnetError,
};

/// What an agent asks the controller for a sidecar's node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrollmentRequest {
    node: NodeLabel,
    tags: TailnetTags,
    state: StateVolume,
}

impl EnrollmentRequest {
    /// A request for the node `node`, tagged `tags`, whose state volume is `state`.
    #[must_use]
    pub const fn new(node: NodeLabel, tags: TailnetTags, state: StateVolume) -> Self {
        Self { node, tags, state }
    }

    /// The hostname of the node.
    #[must_use]
    pub const fn node(&self) -> &NodeLabel {
        &self.node
    }

    /// The ACL tags the node asks for.
    #[must_use]
    pub const fn tags(&self) -> &TailnetTags {
        &self.tags
    }

    /// What the agent found in the state volume.
    #[must_use]
    pub const fn state(&self) -> StateVolume {
        self.state
    }
}

/// What the controller answers: the tailnet's name, and a key when one was minted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    tailnet: TailnetName,
    enrolled: Enrolled,
}

impl Grant {
    /// The `enrolled` outcome for a node of `tailnet`.
    #[must_use]
    pub const fn new(tailnet: TailnetName, enrolled: Enrolled) -> Self {
        Self { tailnet, enrolled }
    }

    /// The `MagicDNS` suffix of the tailnet.
    #[must_use]
    pub const fn tailnet(&self) -> &TailnetName {
        &self.tailnet
    }

    /// Whether a key was minted.
    #[must_use]
    pub const fn enrolled(&self) -> &Enrolled {
        &self.enrolled
    }
}

/// Whoever answers an [`EnrollmentRequest`]: the controller itself, or a client that asks it.
pub trait SidecarEnroller {
    /// Enrol the node of `request`.
    ///
    /// # Errors
    ///
    /// Returns an [`EnrollError`] when the node cannot be enrolled.
    fn enroll(
        &self,
        request: &EnrollmentRequest,
    ) -> impl Future<Output = Result<Grant, EnrollError>> + Send;
}

/// What the agent finds in a project's sidecar state volume.
pub trait StateProbe {
    /// The state of the sidecar of `project`.
    ///
    /// # Errors
    ///
    /// Returns a [`Reason`] when the volume could not be inspected.
    fn probe(
        &self,
        project: &ProjectName,
    ) -> impl Future<Output = Result<StateVolume, Reason>> + Send;
}

/// The Tailscale API: names the tailnet.
pub trait TailnetNames {
    /// The `MagicDNS` suffix of the tailnet the credentials belong to.
    ///
    /// # Errors
    ///
    /// Returns a [`TailnetError`] when it cannot be discovered.
    fn tailnet(&self) -> impl Future<Output = Result<TailnetName, TailnetError>> + Send;
}

/// The state an agent reported, as [`Enrollment`] reads it.
#[derive(Debug, Clone, Copy)]
struct Reported(StateVolume);

impl SidecarState for Reported {
    fn volume(&self, _node: &HostName) -> impl Future<Output = Result<StateVolume, Reason>> + Send {
        std::future::ready(Ok(self.0))
    }
}

/// The controller's use case: discovers the tailnet and mints a key for a node that has none yet,
/// for the tags the operator allows.
#[derive(Debug)]
pub struct Enroller<Names, Minter> {
    names: Names,
    minter: Minter,
    allowed: TailnetTags,
    tailnet: OnceLock<TailnetName>,
}

impl<Names, Minter> Enroller<Names, Minter> {
    /// An enroller that names the tailnet with `names` and mints keys with `minter`, for the
    /// stack tags in `allowed`.
    #[must_use]
    pub const fn new(names: Names, minter: Minter, allowed: TailnetTags) -> Self {
        Self {
            names,
            minter,
            allowed,
            tailnet: OnceLock::new(),
        }
    }

    fn check_allowed(&self, tags: &TailnetTags) -> Result<(), EnrollError> {
        let permitted = |tag: &TailnetTag| self.allowed.iter().any(|allowed| allowed == tag);
        let refused = tags.iter().find(|tag| !permitted(tag));
        let error = |tag: &TailnetTag| EnrollError::Tags(TagError::NotAllowed(tag.clone()));
        refused.map_or(Ok(()), |tag| Err(error(tag)))
    }
}

impl<Names: TailnetNames + Sync, Minter: KeyMinter + Sync> Enroller<Names, Minter> {
    /// The tailnet's name, discovered once.
    async fn discover(&self) -> Result<TailnetName, EnrollError> {
        let known = self.tailnet.get().cloned();
        match known {
            Some(name) => Ok(name),
            None => self.learn().await,
        }
    }

    async fn learn(&self) -> Result<TailnetName, EnrollError> {
        let found = self.names.tailnet().await.map_err(EnrollError::Tailnet)?;
        Ok(self.tailnet.get_or_init(|| found).clone())
    }
}

impl<Names: TailnetNames + Sync, Minter: KeyMinter + Sync> SidecarEnroller
    for Enroller<Names, Minter>
{
    async fn enroll(&self, request: &EnrollmentRequest) -> Result<Grant, EnrollError> {
        self.check_allowed(request.tags())?;
        let node = HostName::new(request.node().as_str()).map_err(EnrollError::Node)?;
        let tailnet = self.discover().await?;
        let enrollment = Enrollment::new(Reported(request.state()), &self.minter);
        let enrolled = enrollment.enroll(&node, request.tags()).await?;
        Ok(Grant::new(tailnet, enrolled))
    }
}
