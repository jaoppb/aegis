//! The use cases of removal: sweeping the stacks the repository no longer declares, and carrying
//! out a tombstone.

use std::future::Future;

use aegis_domain::{
    Deployment, Detail, HostName, HostPaths, Leftover, ProjectNames, RepoPath, StackReport,
    StackReports, Tombstone,
};

use crate::removal::{NodeDeleter, Projects, Purger, SafePath, Teardown};
use crate::source::Reason;

/// What the reconciler asks of removal.
pub trait Removal {
    /// Take down every leftover project that is not in `declared`, keeping its data, and report
    /// each one as orphaned.
    fn orphans(&self, declared: &ProjectNames) -> impl Future<Output = StackReports> + Send;

    /// Carry out `tombstone`, while the paths in `taken` belong to other stacks or to Aegis.
    fn purge(
        &self,
        tombstone: &Tombstone,
        taken: &HostPaths,
    ) -> impl Future<Output = Deployment> + Send;
}

/// Removal that is not configured: it keeps everything and refuses to purge.
#[derive(Debug, Clone, Copy, Default)]
pub struct Retain;

impl Removal for Retain {
    fn orphans(&self, _declared: &ProjectNames) -> impl Future<Output = StackReports> + Send {
        std::future::ready(StackReports::default())
    }

    fn purge(
        &self,
        _tombstone: &Tombstone,
        _taken: &HostPaths,
    ) -> impl Future<Output = Deployment> + Send {
        let detail = Detail::new("purging is not enabled on this agent");
        std::future::ready(Deployment::Failed(detail))
    }
}

fn failed(step: &str, reason: &Reason) -> Detail {
    Detail::new(&format!("{step}: {reason}"))
}

/// Takes down the projects the repository no longer declares. It holds no way to delete data.
#[derive(Debug)]
pub struct OrphanSweep<Lister, Down> {
    host: HostName,
    lister: Lister,
    down: Down,
}

impl<Lister, Down> OrphanSweep<Lister, Down> {
    /// A sweep of `host` that lists projects with `lister` and stops them with `down`.
    #[must_use]
    pub const fn new(host: HostName, lister: Lister, down: Down) -> Self {
        Self { host, lister, down }
    }
}

impl<Lister: Projects + Sync, Down: Teardown + Sync> OrphanSweep<Lister, Down> {
    async fn sweep(&self, declared: &ProjectNames) -> StackReports {
        let found = match self.lister.leftovers().await {
            Ok(found) => found,
            Err(reason) => return self.unreadable(&reason),
        };
        let undeclared = found.iter().filter(|each| !is_declared(each, declared));
        let reports = take_down_all(&self.down, &self.host, undeclared.collect()).await;
        StackReports::new(reports)
    }

    fn unreadable(&self, reason: &Reason) -> StackReports {
        let host = RepoPath::hosts_root().listed(self.host.as_str());
        let detail = failed("cannot list the leftover projects", reason);
        StackReports::new(vec![StackReport::new(host, Deployment::Failed(detail))])
    }
}

async fn take_down_all<Down: Teardown + Sync>(
    down: &Down,
    host: &HostName,
    leftovers: Vec<&Leftover>,
) -> Vec<StackReport> {
    let mut reports = Vec::new();
    for leftover in leftovers {
        reports.push(take_down(down, host, leftover).await);
    }
    reports
}

async fn take_down<Down: Teardown + Sync>(
    down: &Down,
    host: &HostName,
    leftover: &Leftover,
) -> StackReport {
    let stack = leftover.project().stack_name();
    let directory = RepoPath::hosts_root().listed(host.as_str()).listed(stack);
    let deployment = match down.down(leftover.project()).await {
        Ok(()) => Deployment::Orphaned(leftover.remains().clone()),
        Err(reason) => Deployment::Failed(failed("compose down", &reason)),
    };
    StackReport::new(directory, deployment)
}

fn is_declared(leftover: &Leftover, declared: &ProjectNames) -> bool {
    declared.iter().any(|name| name == leftover.project())
}

/// Carries out tombstones: down, the tailnet node, the volumes, then the paths.
#[derive(Debug)]
pub struct Purge<Down, Nodes, Wipe> {
    down: Down,
    nodes: Nodes,
    wipe: Wipe,
    protected: HostPaths,
}

impl<Down, Nodes, Wipe> Purge<Down, Nodes, Wipe> {
    /// A purge that stops projects with `down`, removes nodes with `nodes` and deletes data with
    /// `wipe`.
    #[must_use]
    pub const fn new(down: Down, nodes: Nodes, wipe: Wipe) -> Self {
        Self {
            down,
            nodes,
            wipe,
            protected: HostPaths::new(Vec::new()),
        }
    }

    /// The same purge that never deletes `protected` paths or anything inside or above them, such
    /// as the checkout.
    #[must_use]
    pub fn protecting(self, protected: HostPaths) -> Self {
        Self { protected, ..self }
    }
}

fn safe_paths(tombstone: &Tombstone, taken: &HostPaths) -> Result<Vec<SafePath>, Detail> {
    let paths = tombstone.paths();
    let checked = paths.iter().map(|path| SafePath::check(path, taken));
    let refuse = |refusal: crate::removal::Refusal| Detail::new(&refusal.to_string());
    checked.collect::<Result<Vec<_>, _>>().map_err(refuse)
}

impl<Down: Teardown + Sync, Nodes: NodeDeleter + Sync, Wipe: Purger + Sync>
    Purge<Down, Nodes, Wipe>
{
    async fn run(&self, tombstone: &Tombstone, taken: &HostPaths) -> Result<(), Detail> {
        let everything = taken.iter().chain(self.protected.iter()).cloned();
        let paths = safe_paths(tombstone, &HostPaths::new(everything.collect()))?;
        let down = self.down.down(tombstone.project()).await;
        down.map_err(|reason| failed("compose down", &reason))?;
        remove_node(&self.nodes, tombstone).await?;
        let volumes = self.wipe.remove_volumes(tombstone).await;
        volumes.map_err(|reason| failed("removing volumes", &reason))?;
        remove_all(&self.wipe, tombstone, &paths).await
    }

    async fn carry_out(&self, tombstone: &Tombstone, taken: &HostPaths) -> Deployment {
        match self.run(tombstone, taken).await {
            Ok(()) => Deployment::Purged,
            Err(detail) => Deployment::Failed(detail),
        }
    }
}

async fn remove_node<Nodes: NodeDeleter + Sync>(
    nodes: &Nodes,
    tombstone: &Tombstone,
) -> Result<(), Detail> {
    let Some(node) = tombstone.node() else {
        return Ok(());
    };
    let removed = nodes.delete_node(&node).await;
    removed.map_err(|reason| failed("removing the tailnet node", &reason))
}

async fn remove_all<Wipe: Purger + Sync>(
    wipe: &Wipe,
    tombstone: &Tombstone,
    paths: &[SafePath],
) -> Result<(), Detail> {
    for path in paths {
        let removed = wipe.remove_path(tombstone, path).await;
        removed.map_err(|reason| failed("removing a path", &reason))?;
    }
    Ok(())
}

/// Removal made of a sweep and a purge.
#[derive(Debug)]
pub struct Remover<Sweep, Wipe> {
    sweep: Sweep,
    purge: Wipe,
}

impl<Sweep, Wipe> Remover<Sweep, Wipe> {
    /// Removal that sweeps with `sweep` and purges with `purge`.
    #[must_use]
    pub const fn new(sweep: Sweep, purge: Wipe) -> Self {
        Self { sweep, purge }
    }
}

impl<Lister, Down, Nodes, Wipe, Down2> Removal
    for Remover<OrphanSweep<Lister, Down>, Purge<Down2, Nodes, Wipe>>
where
    Lister: Projects + Sync,
    Down: Teardown + Sync,
    Down2: Teardown + Sync,
    Nodes: NodeDeleter + Sync,
    Wipe: Purger + Sync,
{
    async fn orphans(&self, declared: &ProjectNames) -> StackReports {
        self.sweep.sweep(declared).await
    }

    async fn purge(&self, tombstone: &Tombstone, taken: &HostPaths) -> Deployment {
        self.purge.carry_out(tombstone, taken).await
    }
}
