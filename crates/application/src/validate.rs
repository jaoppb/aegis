//! The validation rules for a loaded stack: mounts, builds and encrypted secret files.

use aegis_domain::{
    Access, DesiredState, EnvFile, FileRole, Host, Hosts, Line, Location, Mount, Presence, Problem,
    Rejection, Rejections, RepoPath, Service, ServiceName, Stack, Stacks, Violation, Violations,
};

use crate::loader::unreadable;
use crate::outcome::Outcome;
use crate::source::RepoSource;

const COMPOSE_FILE: &str = "compose.yaml";

/// Tells whether a file's text is SOPS-encrypted. It checks the format only and never decrypts.
pub trait EncryptionCheck {
    /// Whether `text`, the contents of `file`, carries SOPS metadata.
    fn is_encrypted(&self, file: &RepoPath, text: &str) -> bool;
}

/// The two ports the rules need.
#[derive(Clone, Copy)]
struct Ports<'ports> {
    source: &'ports dyn RepoSource,
    check: &'ports dyn EncryptionCheck,
}

/// A stack being checked and where it lives.
struct Context<'ctx> {
    ports: Ports<'ctx>,
    stack: &'ctx Stack,
    dir: RepoPath,
}

/// A file a compose file points at.
struct Reference<'file> {
    path: &'file RepoPath,
    role: FileRole,
    presence: Presence,
}

/// The stacks that passed and the ones that did not.
struct Judged {
    kept: Stacks,
    rejected: Rejections,
}

/// Check every stack of `outcome` against the rules and move the failing ones to its rejections.
///
/// Stacks that were already rejected stay rejected, so the result lists every problem found.
#[must_use]
pub fn validate_desired_state(
    outcome: &Outcome,
    source: &dyn RepoSource,
    check: &dyn EncryptionCheck,
) -> Outcome {
    let ports = Ports { source, check };
    let state = outcome.state();
    let mut hosts = Vec::new();
    let mut rejected: Vec<Rejection> = Vec::new();
    for host in state.hosts() {
        let result = judge_host(ports, host);
        hosts.push(Host::new(host.name().clone(), result.kept));
        rejected.extend(result.rejected);
    }
    let reserved = judge(ports, &RepoPath::reserved_root(), state.reserved());
    rejected.extend(reserved.rejected);
    let validated = DesiredState::new(Hosts::new(hosts), reserved.kept);
    let earlier = outcome.rejected().iter().cloned();
    Outcome::new(validated, earlier.chain(rejected).collect())
}

fn judge_host(ports: Ports<'_>, host: &Host) -> Judged {
    let dir = RepoPath::hosts_root().listed(host.name().as_str());
    judge(ports, &dir, host.stacks())
}

fn judge(ports: Ports<'_>, parent: &RepoPath, stacks: &Stacks) -> Judged {
    let mut kept = Vec::new();
    let mut rejected = Vec::new();
    for stack in stacks {
        let dir = parent.listed(stack.name().as_str());
        match judge_stack(Context { ports, stack, dir }) {
            Ok(stack) => kept.push(stack.clone()),
            Err(rejection) => rejected.push(rejection),
        }
    }
    Judged {
        kept: Stacks::new(kept),
        rejected: Rejections::new(rejected),
    }
}

fn judge_stack(context: Context<'_>) -> Result<&Stack, Rejection> {
    let violations = stack_violations(&context);
    if violations.is_empty() {
        return Ok(context.stack);
    }
    Err(Rejection::new(context.dir, Violations::new(violations)))
}

fn stack_violations(context: &Context<'_>) -> Vec<Violation> {
    let mut found = Vec::new();
    for service in context.stack.compose().services() {
        found.extend(mount_violations(context, service));
        found.extend(build_violations(context, service));
    }
    found.extend(file_violations(context));
    found
}

impl Context<'_> {
    fn violation(&self, needle: &str, problem: Problem) -> Violation {
        let text = self.stack.compose_source().as_str();
        let file = self.dir.listed(COMPOSE_FILE);
        Violation::new(Location::file(file).at(Line::of(text, needle)), problem)
    }

    /// Whether `relative`, a path from the compose file, exists in the repository.
    fn present(&self, relative: &RepoPath) -> Result<bool, Violation> {
        let source = self.ports.source;
        let contains = |full: RepoPath| source.contains(&full).map_err(|error| unreadable(&error));
        self.dir.within(relative).map_or(Ok(false), contains)
    }
}

fn mount_violations(context: &Context<'_>, service: &Service) -> Vec<Violation> {
    let mut found = Vec::new();
    for mount in service.mounts() {
        found.extend(undeclared_volume(context, service.name(), mount));
        found.extend(relative_mount(context, service.name(), mount));
    }
    found
}

fn undeclared_volume(
    context: &Context<'_>,
    service: &ServiceName,
    mount: &Mount,
) -> Option<Violation> {
    let Mount::Named { volume, .. } = mount else {
        return None;
    };
    if context
        .stack
        .compose()
        .volumes()
        .iter()
        .any(|known| known == volume)
    {
        return None;
    }
    let problem = Problem::UndeclaredVolume {
        service: service.clone(),
        volume: volume.clone(),
    };
    Some(context.violation(volume.as_str(), problem))
}

fn relative_mount(context: &Context<'_>, service: &ServiceName, mount: &Mount) -> Vec<Violation> {
    let Mount::Relative { source, access, .. } = mount else {
        return Vec::new();
    };
    let service = service.clone();
    let source_path = source.clone();
    let mut found = Vec::new();
    if *access != Access::ReadOnly {
        let problem = Problem::WritableRelativeMount {
            service: service.clone(),
            source: source_path.clone(),
        };
        found.push(context.violation(source.as_str(), problem));
    }
    let missing = Problem::MissingMountSource {
        service,
        source: source_path,
    };
    found.extend(absent(context, source, missing));
    found
}

/// A violation for `problem` when `path` is not in the repository, or when reading it fails.
fn absent(context: &Context<'_>, path: &RepoPath, problem: Problem) -> Option<Violation> {
    match context.present(path) {
        Ok(true) => None,
        Ok(false) => Some(context.violation(path.as_str(), problem)),
        Err(violation) => Some(violation),
    }
}

fn build_violations(context: &Context<'_>, service: &Service) -> Vec<Violation> {
    let Some(build) = service.source().build() else {
        return Vec::new();
    };
    let name = service.name();
    let outside = |path: &aegis_domain::DeclaredPath| {
        let problem = Problem::BuildOutsideStack {
            service: name.clone(),
            path: path.clone(),
        };
        context.violation(path.as_str(), problem)
    };
    let Some(inside) = build.context_inside_stack() else {
        return vec![outside(build.context())];
    };
    let mut found = Vec::new();
    let missing = Problem::MissingBuildContext {
        service: name.clone(),
        context: inside.clone(),
    };
    found.extend(absent(context, &inside, missing));
    if let (true, Some(dockerfile)) = (build.dockerfile_escapes(), build.dockerfile()) {
        found.push(outside(dockerfile));
    }
    found
}

fn file_violations(context: &Context<'_>) -> Vec<Violation> {
    let compose = context.stack.compose();
    let mut found = Vec::new();
    let env_files = compose.services().iter().flat_map(Service::env_files);
    found.extend(env_files.filter_map(|file| secret_violation(context, &env_reference(file))));
    for path in compose.secret_files() {
        let reference = Reference {
            path,
            role: FileRole::Secret,
            presence: Presence::Required,
        };
        found.extend(secret_violation(context, &reference));
    }
    for path in compose.referenced_files() {
        let problem = Problem::MissingFile {
            role: FileRole::Referenced,
            path: path.clone(),
        };
        found.extend(absent(context, path, problem));
    }
    found
}

const fn env_reference(env_file: &EnvFile) -> Reference<'_> {
    Reference {
        path: env_file.path(),
        role: FileRole::EnvFile,
        presence: env_file.presence(),
    }
}

/// A violation when the file is missing (and required) or readable but not SOPS-encrypted.
fn secret_violation(context: &Context<'_>, reference: &Reference<'_>) -> Option<Violation> {
    let full = context.dir.within(reference.path).ok()?;
    let needle = reference.path.as_str();
    let present = match context.present(reference.path) {
        Ok(present) => present,
        Err(violation) => return Some(violation),
    };
    if !present {
        let problem = Problem::MissingFile {
            role: reference.role,
            path: reference.path.clone(),
        };
        return (reference.presence == Presence::Required)
            .then(|| context.violation(needle, problem));
    }
    let text = match context.ports.source.read_file(&full) {
        Ok(text) => text,
        Err(error) => return Some(unreadable(&error)),
    };
    let problem = Problem::NotEncrypted {
        path: reference.path.clone(),
    };
    (!context.ports.check.is_encrypted(&full, &text)).then(|| context.violation(needle, problem))
}
