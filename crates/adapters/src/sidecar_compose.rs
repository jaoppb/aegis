//! A [`ComposeRunner`] that adds the tailscale sidecar and Caddy to a stack that exposes a
//! service.
//!
//! It looks into the sidecar's state volume, asks the controller to enrol the node (a key is minted
//! only while the volume is empty), renders the override onto the volatile [`SecretStore`] and
//! merges it as one more compose file. The key reaches Compose in an environment variable, never
//! in a file or an argument, and the workspace is removed once Compose returns.

use aegis_application::{
    ComposeError, ComposeRunner, DeployRequest, Enrolled, EnrollmentRequest, Exposure, ExtraFiles,
    Grant, KEY_VARIABLE, KeyUse, Reason, SecretStore, SidecarEnroller, SidecarSpec, Slot,
    StateProbe,
};
use aegis_domain::{HostPath, ProjectName, Secret, SlotName};

/// The ports a sidecar deploy works through.
#[derive(Debug)]
pub struct SidecarPorts<Probe, Desk, Store> {
    probe: Probe,
    desk: Desk,
    store: Store,
}

impl<Probe, Desk, Store> SidecarPorts<Probe, Desk, Store> {
    /// Look into the state volume with `probe`, enrol through `desk` and keep the override in
    /// `store`.
    #[must_use]
    pub const fn new(probe: Probe, desk: Desk, store: Store) -> Self {
        Self { probe, desk, store }
    }
}

/// Runs `runner` after adding the sidecar of the stack it deploys, when the stack exposes one.
#[derive(Debug)]
pub struct SidecarCompose<Runner, Probe, Desk, Store> {
    runner: Runner,
    ports: SidecarPorts<Probe, Desk, Store>,
}

impl<Runner, Probe, Desk, Store> SidecarCompose<Runner, Probe, Desk, Store> {
    /// Add the sidecar through `ports`, then run `runner`.
    #[must_use]
    pub const fn new(runner: Runner, ports: SidecarPorts<Probe, Desk, Store>) -> Self {
        Self { runner, ports }
    }
}

fn failure(step: &str, cause: &impl std::fmt::Display) -> ComposeError {
    ComposeError::new(Reason::new(&format!("{step}: {cause}")))
}

/// Whether the override gives the sidecar a key, which it does when one was minted.
const fn key_use(grant: &Grant) -> KeyUse {
    match grant.enrolled() {
        Enrolled::Minted(_) => KeyUse::Fresh,
        Enrolled::AlreadyEnrolled => KeyUse::Reused,
    }
}

impl<Probe, Desk, Store> SidecarPorts<Probe, Desk, Store>
where
    Probe: StateProbe + Sync,
    Desk: SidecarEnroller + Sync,
    Store: SecretStore + Sync,
{
    async fn grant(
        &self,
        request: &DeployRequest,
        exposure: &Exposure,
    ) -> Result<Grant, ComposeError> {
        let project = request.target().project();
        let state = self.probe.probe(project).await;
        let state = state.map_err(|error| failure("cannot check the sidecar state", &error))?;
        let node = exposure
            .node()
            .map_err(|error| failure("cannot name the sidecar node", &error))?;
        let asked = EnrollmentRequest::new(node, exposure.tags().clone(), state);
        let granted = self.desk.enroll(&asked).await;
        granted.map_err(|error| failure("cannot enrol the sidecar", &error))
    }

    async fn place(&self, project: &ProjectName, text: &str) -> Result<HostPath, ComposeError> {
        let name = SlotName::new("sidecar.yaml").map_err(|error| failure("slot", &error))?;
        let slot = Slot::new(project.clone(), name);
        let placed = self.store.place(&slot, &Secret::new(text)).await;
        placed.map_err(|error| failure("cannot keep the sidecar override", &error))
    }

    /// `request` with the sidecar override merged in, and the key in its environment.
    async fn prepare(
        &self,
        request: &DeployRequest,
        exposure: &Exposure,
    ) -> Result<DeployRequest, ComposeError> {
        let grant = self.grant(request, exposure).await?;
        let spec = SidecarSpec::new(exposure, grant.tailnet().clone())
            .map_err(|error| failure("cannot render the sidecar", &error))?;
        let rendered = spec.render(key_use(&grant));
        let project = request.target().project();
        let path = self.place(project, rendered.compose().as_str()).await?;
        Ok(with_sidecar(request, path, &grant))
    }
}

fn with_sidecar(request: &DeployRequest, path: HostPath, grant: &Grant) -> DeployRequest {
    let extras = request.extras();
    let mut files = extras.files().as_slice().to_vec();
    files.push(path);
    let environment = extras.environment().clone();
    let environment = match grant.enrolled() {
        Enrolled::Minted(key) => environment.with_variable(KEY_VARIABLE, key.expose()),
        Enrolled::AlreadyEnrolled => environment,
    };
    let merged = extras
        .clone()
        .with_files(ExtraFiles::new(files))
        .with_environment(environment);
    request.clone().with_extras(merged)
}

impl<Runner, Probe, Desk, Store> SidecarCompose<Runner, Probe, Desk, Store>
where
    Runner: ComposeRunner + Sync,
    Probe: StateProbe + Sync,
    Desk: SidecarEnroller + Sync,
    Store: SecretStore + Sync,
{
    async fn expose(
        &self,
        request: &DeployRequest,
        exposure: &Exposure,
    ) -> Result<(), ComposeError> {
        let prepared = self.ports.prepare(request, exposure).await;
        let outcome = match &prepared {
            Ok(ready) => self.runner.converge(ready).await,
            Err(error) => Err(error.clone()),
        };
        let removed = self.ports.store.discard(request.target().project()).await;
        outcome?;
        removed.map_err(|error| failure("cannot remove the sidecar override", &error))
    }
}

impl<Runner, Probe, Desk, Store> ComposeRunner for SidecarCompose<Runner, Probe, Desk, Store>
where
    Runner: ComposeRunner + Sync,
    Probe: StateProbe + Sync,
    Desk: SidecarEnroller + Sync,
    Store: SecretStore + Sync,
{
    async fn converge(&self, request: &DeployRequest) -> Result<(), ComposeError> {
        match request.target().exposure() {
            Some(exposure) => self.expose(request, exposure).await,
            None => self.runner.converge(request).await,
        }
    }
}
