//! Summarises a `compose.yaml`: only the services, volumes, builds, env files and mounts.

use std::collections::BTreeMap;
use std::fmt::Display;

use aegis_application::ParseError;
use aegis_domain::{
    Access, AdditionalContexts, Build, ComposeSummary, ContainerPath, DeclaredPath, EnvFile,
    EnvFiles, FileReference, HostPath, ImageRef, Line, Mount, Mounts, Presence, ReferenceKind,
    ReferencedFiles, RepoPath, SecretFile, SecretFiles, SecretName, Service, ServiceName,
    ServiceSource, Services, ValueError, VolumeName, Volumes,
};
use serde::Deserialize;

use crate::allowlist;

#[derive(Deserialize)]
struct RawCompose {
    services: Option<BTreeMap<String, RawService>>,
    volumes: Option<BTreeMap<String, Option<RawVolume>>>,
    #[serde(flatten)]
    files: RawFiles,
}

#[derive(Deserialize)]
struct RawFiles {
    secrets: Option<BTreeMap<String, RawFileSource>>,
    configs: Option<BTreeMap<String, RawFileSource>>,
    include: Option<Vec<RawInclude>>,
}

#[derive(Deserialize)]
struct RawVolume {
    driver_opts: Option<BTreeMap<String, serde_norway::Value>>,
}

#[derive(Deserialize)]
struct RawFileSource {
    file: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawInclude {
    Path(String),
    Detailed { path: Option<RawIncludePath> },
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawIncludePath {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawExtends {
    Detailed { file: Option<String> },
    Service(serde::de::IgnoredAny),
}

#[derive(Deserialize)]
struct RawService {
    #[serde(flatten)]
    source: RawSource,
    env_file: Option<RawEnvFile>,
    volumes: Option<Vec<RawMount>>,
    extends: Option<RawExtends>,
}

#[derive(Deserialize)]
struct RawSource {
    image: Option<String>,
    build: Option<RawBuild>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawBuild {
    Context(String),
    Detailed {
        context: Option<String>,
        dockerfile: Option<String>,
        additional_contexts: Option<RawAdditionalContexts>,
    },
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawAdditionalContexts {
    Named(BTreeMap<String, String>),
    Listed(Vec<String>),
}

impl RawAdditionalContexts {
    /// The values of the contexts, which are paths, URLs or `service:` references.
    fn values(self) -> Vec<String> {
        match self {
            Self::Named(named) => named.into_values().collect(),
            Self::Listed(listed) => listed.iter().map(String::as_str).map(entry_value).collect(),
        }
    }
}

/// The value of a `name=value` entry; an entry without `=` is all value.
fn entry_value(entry: &str) -> String {
    let (_, value) = entry.split_once('=').unwrap_or(("", entry));
    value.to_owned()
}

/// Contexts that are references, not paths in the checkout.
const REFERENCE_PREFIXES: &[&str] = &[
    "service:",
    "docker-image://",
    "github.com/",
    "https://",
    "http://",
    "git://",
];

/// Whether `value` is the scp form `git@host:path`: a host without a `/` before the first colon,
/// as `BuildKit` reads it. Anything else starting with `git@` is a local path.
fn is_scp_git(value: &str) -> bool {
    let host = value
        .strip_prefix("git@")
        .and_then(|rest| rest.split_once(':'));
    host.is_some_and(|(name, _)| !name.is_empty() && !name.contains('/'))
}

/// An additional build context: `Some` path in the checkout, `None` for a known reference, and
/// a refusal for any other scheme (`oci-layout://` is a host directory), so the unknown fails closed.
fn additional_context(value: String) -> Result<Option<String>, ParseError> {
    if is_scp_git(&value)
        || REFERENCE_PREFIXES
            .iter()
            .any(|prefix| value.starts_with(prefix))
    {
        return Ok(None);
    }
    if value.contains("://") {
        return Err(ParseError::Invalid(format!(
            "build: additional context `{value}` has a scheme Aegis does not support"
        )));
    }
    Ok(Some(value))
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
    Detailed {
        path: String,
        required: Option<bool>,
    },
}

/// Summarise `text`; an empty document is an empty compose file.
///
/// # Errors
///
/// Returns [`ParseError::Invalid`] when `text` is not YAML, or a service, volume, build, env file,
/// mount or file reference is malformed.
pub fn parse(text: &str) -> Result<ComposeSummary, ParseError> {
    let raw = read_document(text)?;
    let Some(raw) = raw else {
        return Ok(ComposeSummary::default());
    };
    let secrets = secret_files(&raw)?;
    let referenced = referenced_files(&raw)?;
    let services = raw.services.unwrap_or_default().into_iter();
    let summary = ComposeSummary::new(
        services
            .map(|(name, raw)| service(&name, raw))
            .collect::<Result<Services, _>>()?,
        volumes(raw.volumes)?,
    );
    Ok(summary
        .with_secret_files(secrets)
        .with_referenced_files(referenced))
}

fn yaml(error: &serde_norway::Error) -> ParseError {
    let reason = error.to_string();
    match error.location().and_then(|place| Line::new(place.line())) {
        Some(line) => ParseError::InvalidAt { reason, line },
        None => ParseError::Invalid(reason),
    }
}

fn secret_files(raw: &RawCompose) -> Result<SecretFiles, ParseError> {
    let entries = raw.files.secrets.iter().flat_map(BTreeMap::iter);
    let files = entries.filter_map(|(name, source)| Some((name, source.file.as_ref()?)));
    files
        .map(|(name, path)| {
            let name = SecretName::new(name).map_err(|error| in_scope("secrets", &error))?;
            let path = RepoPath::new(path).map_err(|error| in_scope("secrets", &error))?;
            Ok(SecretFile::new(name, path))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(SecretFiles::new)
}

fn reference_paths(
    kind: ReferenceKind,
    paths: impl Iterator<Item = String>,
) -> Result<Vec<FileReference>, ParseError> {
    let scope = "include, extends or configs";
    paths
        .map(|path| {
            let path = RepoPath::new(path).map_err(|error| in_scope(scope, &error))?;
            Ok(FileReference::new(path, kind))
        })
        .collect()
}

fn referenced_files(raw: &RawCompose) -> Result<ReferencedFiles, ParseError> {
    let configs = raw.files.configs.iter().flat_map(BTreeMap::values);
    let configs = configs.filter_map(|source| source.file.clone());
    let includes = raw.files.include.iter().flatten().flat_map(include_paths);
    let services = raw.services.iter().flat_map(BTreeMap::values);
    let extends = services.filter_map(|service| match &service.extends {
        Some(RawExtends::Detailed { file }) => file.clone(),
        _ => None,
    });
    let mut found = reference_paths(ReferenceKind::Config, configs)?;
    found.extend(reference_paths(ReferenceKind::Include, includes)?);
    found.extend(reference_paths(ReferenceKind::Extends, extends)?);
    Ok(ReferencedFiles::new(found))
}

fn include_paths(include: &RawInclude) -> Vec<String> {
    match include {
        RawInclude::Path(path) => vec![path.clone()],
        RawInclude::Detailed { path } => path.as_ref().map_or_else(Vec::new, include_path_list),
    }
}

fn include_path_list(path: &RawIncludePath) -> Vec<String> {
    match path {
        RawIncludePath::One(one) => vec![one.clone()],
        RawIncludePath::Many(many) => many.clone(),
    }
}

/// Read the YAML with its `<<` merge keys applied, as Compose does, so a merged service or volume
/// is checked like any other.
fn read_document(text: &str) -> Result<Option<RawCompose>, ParseError> {
    let mut document: serde_norway::Value =
        serde_norway::from_str(text).map_err(|error| yaml(&error))?;
    refuse(
        "YAML tags are not supported: remove the tag at",
        &allowlist::tags(&document),
    )?;
    document
        .apply_merge()
        .map_err(|error| ParseError::Invalid(error.to_string()))?;
    refuse(
        "chained YAML merge keys (`<<`) are not supported: write the merged content out at",
        &allowlist::merge_keys(&document),
    )?;
    refuse(
        "keys Aegis does not support (remove them or use an `x-` extension key):",
        &allowlist::unsupported_keys(&document),
    )?;
    serde_norway::from_value(document).map_err(|error| yaml(&error))
}

/// Fail with every path in `found`, or pass when there is none.
fn refuse(reason: &str, found: &[String]) -> Result<(), ParseError> {
    if found.is_empty() {
        return Ok(());
    }
    let paths: Vec<String> = found.iter().map(|path| format!("`{path}`")).collect();
    Err(ParseError::Invalid(format!(
        "{reason} {}",
        paths.join(", ")
    )))
}

fn volumes(raw: Option<BTreeMap<String, Option<RawVolume>>>) -> Result<Volumes, ParseError> {
    let declared = raw.unwrap_or_default();
    declared
        .into_iter()
        .map(|(name, volume)| {
            reject_unsafe_driver_options(&name, volume.as_ref())?;
            VolumeName::new(name).map_err(|error| in_scope("volumes", &error))
        })
        .collect()
}

/// Whether `driver_opts` mount a device as a bind: `type: none` or `o` holding `bind`.
fn binds_a_device(options: &BTreeMap<String, serde_norway::Value>) -> bool {
    let text = |key: &str| options.get(key).and_then(serde_norway::Value::as_str);
    text("type") == Some("none") || text("o").is_some_and(|mount| mount.contains("bind"))
}

fn is_absolute_path(device: &serde_norway::Value) -> bool {
    device.as_str().is_some_and(|path| path.starts_with('/'))
}

/// Refuse the driver options the rules cannot check: an interpolated one, and a bind of a device
/// that is not an absolute host path (a bind mount of a path the repository or the home directory
/// decides, which the mount rules cannot see). A number is not an absolute path either.
fn reject_unsafe_driver_options(name: &str, raw: Option<&RawVolume>) -> Result<(), ParseError> {
    let options = raw.and_then(|volume| volume.driver_opts.as_ref());
    let Some(options) = options else {
        return Ok(());
    };
    if options.values().any(is_interpolated) {
        return Err(ParseError::Invalid(format!(
            "volume `{name}` interpolates a driver option (`$`): write the value out"
        )));
    }
    let acceptable = !binds_a_device(options) || options.get("device").is_none_or(is_absolute_path);
    if acceptable {
        return Ok(());
    }
    Err(ParseError::Invalid(format!(
        "volume `{name}` binds a device that is not an absolute host path: use an absolute host path"
    )))
}

/// Compose interpolates driver options, so a `$` hides what the bind rules would check.
fn is_interpolated(value: &serde_norway::Value) -> bool {
    value.as_str().is_some_and(|text| text.contains('$'))
}

fn in_scope(scope: &str, error: &impl Display) -> ParseError {
    ParseError::Invalid(format!("{scope}: {error}"))
}

fn service(name: &str, raw: RawService) -> Result<Service, ParseError> {
    let scope = format!("service `{name}`");
    let scoped = |error: &dyn Display| in_scope(&scope, &error);
    let mounts = raw.volumes.unwrap_or_default().into_iter();
    let image = raw.source.image.map(ImageRef::new).transpose();
    let build = raw.source.build.map(build).transpose();
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
        RawBuild::Context(context) => detailed_build(Some(context), None, None),
        RawBuild::Detailed {
            context,
            dockerfile,
            additional_contexts,
        } => detailed_build(context, dockerfile, additional_contexts),
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
    additional: Option<RawAdditionalContexts>,
) -> Result<Build, ParseError> {
    let context = context.unwrap_or_else(|| ".".to_owned());
    reject_remote(&context)?;
    let invalid = |error: ValueError| ParseError::Invalid(format!("build: {error}"));
    let extra = additional
        .map(RawAdditionalContexts::values)
        .unwrap_or_default();
    let extra = extra.into_iter().map(additional_context);
    let extra = extra.collect::<Result<Vec<_>, _>>()?.into_iter().flatten();
    let extra = extra
        .map(DeclaredPath::new)
        .collect::<Result<AdditionalContexts, _>>()
        .map_err(invalid)?;
    let dockerfile = dockerfile
        .map(DeclaredPath::new)
        .transpose()
        .map_err(invalid)?;
    let context = DeclaredPath::new(context).map_err(invalid)?;
    Ok(Build::new(context, dockerfile).with_additional_contexts(extra))
}

fn env_files(raw: Option<RawEnvFile>) -> Result<EnvFiles, ValueError> {
    let entries = match raw {
        None => Vec::new(),
        Some(RawEnvFile::One(path)) => vec![RawEnvEntry::Path(path)],
        Some(RawEnvFile::Many(entries)) => entries,
    };
    entries.into_iter().map(env_file).collect()
}

fn env_file(entry: RawEnvEntry) -> Result<EnvFile, ValueError> {
    let (path, required) = match entry {
        RawEnvEntry::Path(path) => (path, None),
        RawEnvEntry::Detailed { path, required } => (path, required),
    };
    let presence = if required == Some(false) {
        Presence::Optional
    } else {
        Presence::Required
    };
    Ok(EnvFile::new(RepoPath::new(path)?, presence))
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

/// The last `ro` or `rw` option wins, as in Compose.
fn short_access(options: &str) -> Access {
    let last = options
        .split(',')
        .rfind(|option| matches!(*option, "ro" | "rw"));
    if last == Some("ro") {
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
        ("volume", Some(source)) => with_source(&source, &long.target, access),
        ("bind", Some(source)) => bind_source(&source, &long.target, access),
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

/// A bind mount is a host path when absolute and a path in the repository otherwise, however the
/// source starts: Compose resolves `data` against the project directory.
fn bind_source(source: &str, target: &str, access: Access) -> Result<Mount, ParseError> {
    let target = container(target)?;
    if source.starts_with('/') {
        return host_path(source, target, access);
    }
    relative(source, target, access)
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
