//! Summarises a `compose.yaml`: only the services, volumes, builds, env files and mounts.

use std::collections::BTreeMap;
use std::fmt::Display;

use aegis_application::ParseError;
use aegis_domain::{
    Access, Build, ComposeSummary, ContainerPath, EnvFiles, HostPath, ImageRef, Mount, Mounts,
    RepoPath, Service, ServiceName, ServiceSource, Services, ValueError, VolumeName, Volumes,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct RawCompose {
    services: Option<BTreeMap<String, RawService>>,
    volumes: Option<BTreeMap<String, serde_norway::Value>>,
}

#[derive(Deserialize)]
struct RawService {
    image: Option<String>,
    build: Option<RawBuild>,
    env_file: Option<RawEnvFile>,
    volumes: Option<Vec<RawMount>>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawBuild {
    Context(String),
    Detailed {
        context: Option<String>,
        dockerfile: Option<String>,
    },
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawEnvFile {
    One(String),
    Many(Vec<RawEnvEntry>),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawEnvEntry {
    Path(String),
    Detailed { path: String },
}

/// Summarise `text`; an empty document is an empty compose file.
///
/// # Errors
///
/// Returns [`ParseError::Invalid`] when `text` is not YAML, or a service, volume, build, env file
/// or mount is malformed.
pub fn parse(text: &str) -> Result<ComposeSummary, ParseError> {
    let raw: Option<RawCompose> =
        serde_norway::from_str(text).map_err(|error| ParseError::Invalid(error.to_string()))?;
    let Some(raw) = raw else {
        return Ok(ComposeSummary::default());
    };
    let services = raw.services.unwrap_or_default().into_iter();
    let volumes = raw.volumes.unwrap_or_default().into_keys();
    Ok(ComposeSummary::new(
        services
            .map(|(name, raw)| service(&name, raw))
            .collect::<Result<Services, _>>()?,
        volumes.map(volume).collect::<Result<Volumes, _>>()?,
    ))
}

fn volume(name: String) -> Result<VolumeName, ParseError> {
    VolumeName::new(name).map_err(|error| in_scope("volumes", &error))
}

fn in_scope(scope: &str, error: &impl Display) -> ParseError {
    ParseError::Invalid(format!("{scope}: {error}"))
}

fn service(name: &str, raw: RawService) -> Result<Service, ParseError> {
    let scope = format!("service `{name}`");
    let scoped = |error: &dyn Display| in_scope(&scope, &error);
    let mounts = raw.volumes.unwrap_or_default().into_iter();
    let image = raw.image.map(ImageRef::new).transpose();
    let build = raw.build.map(build).transpose();
    let source = ServiceSource::new(
        image.map_err(|error| scoped(&error))?,
        build.map_err(|error| scoped(&error))?,
    );
    let name = ServiceName::new(name).map_err(|error| scoped(&error))?;
    Ok(Service::new(name, source)
        .with_env_files(env_files(raw.env_file).map_err(|error| scoped(&error))?)
        .with_mounts(
            mounts
                .map(mount)
                .collect::<Result<Mounts, _>>()
                .map_err(|error| scoped(&error))?,
        ))
}

fn build(raw: RawBuild) -> Result<Build, ParseError> {
    match raw {
        RawBuild::Context(context) => detailed_build(Some(context), None),
        RawBuild::Detailed {
            context,
            dockerfile,
        } => detailed_build(context, dockerfile),
    }
}

/// Builds come from the checkout, so a URL or git context is not a path inside the repository.
fn reject_remote(context: &str) -> Result<(), ParseError> {
    if context.contains("://") || context.starts_with("git@") || context.starts_with("github.com/")
    {
        return Err(ParseError::Invalid(
            "remote build contexts are not supported".to_owned(),
        ));
    }
    Ok(())
}

fn detailed_build(
    context: Option<String>,
    dockerfile: Option<String>,
) -> Result<Build, ParseError> {
    let context = context.unwrap_or_else(|| ".".to_owned());
    reject_remote(&context)?;
    let invalid = |error: ValueError| ParseError::Invalid(format!("build: {error}"));
    Ok(Build::new(
        RepoPath::new(context).map_err(invalid)?,
        dockerfile.map(RepoPath::new).transpose().map_err(invalid)?,
    ))
}

fn env_files(raw: Option<RawEnvFile>) -> Result<EnvFiles, ValueError> {
    let entries = match raw {
        None => Vec::new(),
        Some(RawEnvFile::One(path)) => vec![RawEnvEntry::Path(path)],
        Some(RawEnvFile::Many(entries)) => entries,
    };
    entries.into_iter().map(env_path).collect()
}

fn env_path(entry: RawEnvEntry) -> Result<RepoPath, ValueError> {
    match entry {
        RawEnvEntry::Path(path) | RawEnvEntry::Detailed { path } => RepoPath::new(path),
    }
}

// Mounts.

#[derive(Deserialize)]
#[serde(untagged)]
enum RawMount {
    Short(String),
    Long(RawLongMount),
}

#[derive(Deserialize)]
struct RawLongMount {
    #[serde(rename = "type")]
    kind: String,
    source: Option<String>,
    target: String,
    read_only: Option<bool>,
}

fn invalid(error: &ValueError) -> ParseError {
    ParseError::Invalid(format!("mount: {error}"))
}

fn mount(raw: RawMount) -> Result<Mount, ParseError> {
    match raw {
        RawMount::Short(spec) => short(&spec),
        RawMount::Long(long) => long_form(long),
    }
}

fn short(spec: &str) -> Result<Mount, ParseError> {
    let parts: Vec<&str> = spec.split(':').collect();
    match parts.as_slice() {
        [target] => anonymous(target),
        [source, target] => with_source(source, target, Access::ReadWrite),
        [source, target, options] => with_source(source, target, short_access(options)),
        _ => Err(ParseError::Invalid(format!(
            "mount `{spec}` has too many parts"
        ))),
    }
}

fn short_access(options: &str) -> Access {
    if options.split(',').any(|option| option == "ro") {
        Access::ReadOnly
    } else {
        Access::ReadWrite
    }
}

fn anonymous(target: &str) -> Result<Mount, ParseError> {
    Ok(Mount::Anonymous {
        target: ContainerPath::new(target).map_err(|error| invalid(&error))?,
    })
}

fn long_form(long: RawLongMount) -> Result<Mount, ParseError> {
    let access = if long.read_only == Some(true) {
        Access::ReadOnly
    } else {
        Access::ReadWrite
    };
    match (long.kind.as_str(), long.source) {
        ("tmpfs", _) => Ok(Mount::Tmpfs {
            target: container(&long.target)?,
        }),
        ("volume" | "bind", Some(source)) => with_source(&source, &long.target, access),
        ("volume", None) => anonymous(&long.target),
        ("bind", None) => Err(ParseError::Invalid(
            "a bind mount needs a source".to_owned(),
        )),
        (other, _) => Err(ParseError::Invalid(format!(
            "unsupported mount type `{other}`"
        ))),
    }
}

fn container(target: &str) -> Result<ContainerPath, ParseError> {
    ContainerPath::new(target).map_err(|error| invalid(&error))
}

/// A mount of `source`: a host path, a path in the repository or a named volume.
fn with_source(source: &str, target: &str, access: Access) -> Result<Mount, ParseError> {
    let target = container(target)?;
    match source.chars().next() {
        Some('/') => host_path(source, target, access),
        Some('.') => relative(source, target, access),
        _ => named(source, target, access),
    }
}

fn host_path(source: &str, target: ContainerPath, access: Access) -> Result<Mount, ParseError> {
    let source = HostPath::new(source).map_err(|error| invalid(&error))?;
    Ok(Mount::HostPath {
        source,
        target,
        access,
    })
}

fn relative(source: &str, target: ContainerPath, access: Access) -> Result<Mount, ParseError> {
    let source = RepoPath::new(source).map_err(|error| invalid(&error))?;
    Ok(Mount::Relative {
        source,
        target,
        access,
    })
}

fn named(source: &str, target: ContainerPath, access: Access) -> Result<Mount, ParseError> {
    let volume = VolumeName::new(source).map_err(|error| invalid(&error))?;
    Ok(Mount::Named {
        volume,
        target,
        access,
    })
}
