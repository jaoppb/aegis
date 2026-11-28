//! The fail-closed part of the compose parser: every YAML tag and every key the model does not
//! understand is reported, because Compose would act on what the typed parse silently drops.

use serde_norway::{Mapping, Value};

/// The compose keys Aegis understands at one place of the document.
struct Rule {
    /// The keys to walk from the document root; `*` is every entry of a mapping or sequence.
    route: &'static [&'static str],
    allowed: &'static [&'static str],
}

const TOP: &[&str] = &[
    "name", "version", "services", "volumes", "networks", "secrets", "configs", "include",
];

/// Spec service keys that read no host path or file, plus the ones the model reads itself.
/// `label_file`, `develop`, `provider` and `credential_spec` are left out on purpose.
const SERVICE: &[&str] = &[
    "image",
    "build",
    "env_file",
    "volumes",
    "extends",
    "annotations",
    "attach",
    "blkio_config",
    "cap_add",
    "cap_drop",
    "cgroup",
    "cgroup_parent",
    "command",
    "configs",
    "container_name",
    "cpu_count",
    "cpu_percent",
    "cpu_period",
    "cpu_quota",
    "cpu_rt_period",
    "cpu_rt_runtime",
    "cpu_shares",
    "cpus",
    "cpuset",
    "depends_on",
    "deploy",
    "device_cgroup_rules",
    "devices",
    "dns",
    "dns_opt",
    "dns_search",
    "domainname",
    "entrypoint",
    "environment",
    "expose",
    "external_links",
    "extra_hosts",
    "gpus",
    "group_add",
    "healthcheck",
    "hostname",
    "init",
    "ipc",
    "isolation",
    "labels",
    "links",
    "logging",
    "mac_address",
    "mem_limit",
    "mem_reservation",
    "mem_swappiness",
    "memswap_limit",
    "network_mode",
    "networks",
    "oom_kill_disable",
    "oom_score_adj",
    "pid",
    "pids_limit",
    "platform",
    "ports",
    "post_start",
    "pre_stop",
    "privileged",
    "profiles",
    "pull_policy",
    "read_only",
    "restart",
    "runtime",
    "scale",
    "secrets",
    "security_opt",
    "shm_size",
    "stdin_open",
    "stop_grace_period",
    "stop_signal",
    "storage_opt",
    "sysctls",
    "tmpfs",
    "tty",
    "ulimits",
    "user",
    "userns_mode",
    "uts",
    "volumes_from",
    "working_dir",
];

/// Build keys without a host path, plus the ones the model reads itself. `ssh`, `cache_from`
/// and `cache_to` can read or write host paths and are left out.
const BUILD: &[&str] = &[
    "context",
    "dockerfile",
    "additional_contexts",
    "args",
    "dockerfile_inline",
    "extra_hosts",
    "isolation",
    "labels",
    "network",
    "no_cache",
    "platforms",
    "pull",
    "secrets",
    "shm_size",
    "tags",
    "target",
    "ulimits",
];

const EXTENDS: &[&str] = &["file", "service"];
const ENV_ENTRY: &[&str] = &["path", "required", "format"];
const MOUNT: &[&str] = &[
    "type",
    "source",
    "target",
    "read_only",
    "bind",
    "volume",
    "tmpfs",
    "consistency",
];
const VOLUME: &[&str] = &["driver", "driver_opts", "external", "labels", "name"];
const SECRET: &[&str] = &["file", "environment", "external", "labels", "name"];
const CONFIG: &[&str] = &[
    "file",
    "content",
    "environment",
    "external",
    "labels",
    "name",
];
const INCLUDE: &[&str] = &["path"];
const NETWORK: &[&str] = &[
    "driver",
    "driver_opts",
    "attachable",
    "enable_ipv4",
    "enable_ipv6",
    "external",
    "internal",
    "ipam",
    "labels",
    "name",
];
const SERVICE_NETWORK: &[&str] = &[
    "aliases",
    "driver_opts",
    "gw_priority",
    "interface_name",
    "ipv4_address",
    "ipv6_address",
    "link_local_ips",
    "mac_address",
    "priority",
];
const SERVICE_SECRET: &[&str] = &["source", "target", "uid", "gid", "mode"];
const BIND: &[&str] = &["propagation", "create_host_path", "selinux", "recursive"];
const VOLUME_OPTIONS: &[&str] = &["nocopy", "subpath", "labels"];
const TMPFS: &[&str] = &["size", "mode"];

const RULES: &[Rule] = &[
    Rule {
        route: &[],
        allowed: TOP,
    },
    Rule {
        route: &["services", "*"],
        allowed: SERVICE,
    },
    Rule {
        route: &["services", "*", "build"],
        allowed: BUILD,
    },
    Rule {
        route: &["services", "*", "extends"],
        allowed: EXTENDS,
    },
    Rule {
        route: &["services", "*", "env_file", "*"],
        allowed: ENV_ENTRY,
    },
    Rule {
        route: &["services", "*", "volumes", "*"],
        allowed: MOUNT,
    },
    Rule {
        route: &["services", "*", "networks", "*"],
        allowed: SERVICE_NETWORK,
    },
    Rule {
        route: &["services", "*", "secrets", "*"],
        allowed: SERVICE_SECRET,
    },
    Rule {
        route: &["services", "*", "configs", "*"],
        allowed: SERVICE_SECRET,
    },
    Rule {
        route: &["services", "*", "build", "secrets", "*"],
        allowed: SERVICE_SECRET,
    },
    Rule {
        route: &["services", "*", "volumes", "*", "bind"],
        allowed: BIND,
    },
    Rule {
        route: &["services", "*", "volumes", "*", "volume"],
        allowed: VOLUME_OPTIONS,
    },
    Rule {
        route: &["services", "*", "volumes", "*", "tmpfs"],
        allowed: TMPFS,
    },
    Rule {
        route: &["networks", "*"],
        allowed: NETWORK,
    },
    Rule {
        route: &["volumes", "*"],
        allowed: VOLUME,
    },
    Rule {
        route: &["secrets", "*"],
        allowed: SECRET,
    },
    Rule {
        route: &["configs", "*"],
        allowed: CONFIG,
    },
    Rule {
        route: &["include", "*"],
        allowed: INCLUDE,
    },
];

/// A node of the document and where it sits, as a dotted path.
struct Member<'doc> {
    path: String,
    node: &'doc Value,
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_owned()
    } else {
        format!("{path}.{key}")
    }
}

fn key_text(key: &Value) -> String {
    key.as_str()
        .map_or_else(|| "<non-text key>".to_owned(), str::to_owned)
}

impl<'doc> Member<'doc> {
    const fn root(node: &'doc Value) -> Self {
        Self {
            path: String::new(),
            node,
        }
    }

    fn at(&self, key: &str) -> Option<Self> {
        let node = self.node.as_mapping()?.get(key)?;
        Some(Self {
            path: join(&self.path, key),
            node,
        })
    }

    /// The entries of a mapping or the items of a sequence.
    fn children(&self) -> Vec<Self> {
        match self.node {
            Value::Mapping(mapping) => mapping
                .iter()
                .map(|(key, node)| Self {
                    path: join(&self.path, &key_text(key)),
                    node,
                })
                .collect(),
            Value::Sequence(items) => items
                .iter()
                .enumerate()
                .map(|(index, node)| Self {
                    path: format!("{}[{index}]", self.path),
                    node,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The members one route step down: every child for `*`, else the named one.
    fn step(&self, key: &str) -> Vec<Self> {
        match key {
            "*" => self.children(),
            named => self.at(named).into_iter().collect(),
        }
    }

    /// The keys of this mapping that are neither allowed nor `x-` extensions.
    fn unsupported(&self, allowed: &[&str]) -> Vec<String> {
        let keys = self.node.as_mapping().into_iter().flat_map(Mapping::keys);
        keys.map(key_text)
            .filter(|key| !is_known(key, allowed))
            .map(|key| join(&self.path, &key))
            .collect()
    }

    /// What `find` reports at this node and at every node below it.
    fn everywhere(&self, find: fn(&Self) -> Vec<String>) -> Vec<String> {
        let below = self.children();
        let inner = below.iter().flat_map(|member| member.everywhere(find));
        find(self).into_iter().chain(inner).collect()
    }

    /// This node's path when it is tagged, and the paths of its tagged keys (`!custom <<: *a`).
    fn tagged(&self) -> Vec<String> {
        let here = matches!(self.node, Value::Tagged(_)).then(|| self.path.clone());
        let keys = self.node.as_mapping().into_iter().flat_map(Mapping::keys);
        let keys = keys.filter(|key| matches!(key, Value::Tagged(_)));
        let keys = keys.map(|key| join(&self.path, &key_text(key)));
        here.into_iter().chain(keys).collect()
    }

    /// The paths of the `<<` keys of this mapping.
    fn merges(&self) -> Vec<String> {
        let keys = self.node.as_mapping().into_iter().flat_map(Mapping::keys);
        keys.filter(|key| key.as_str() == Some("<<"))
            .map(|key| join(&self.path, &key_text(key)))
            .collect()
    }
}

fn is_known(key: &str, allowed: &[&str]) -> bool {
    key.starts_with("x-") || allowed.contains(&key)
}

fn advance<'doc>(level: &[Member<'doc>], key: &str) -> Vec<Member<'doc>> {
    level.iter().flat_map(|member| member.step(key)).collect()
}

fn select<'doc>(node: Member<'doc>, route: &[&str]) -> Vec<Member<'doc>> {
    route
        .iter()
        .fold(vec![node], |level, key| advance(&level, key))
}

impl Rule {
    fn unsupported_in(&self, document: &Value) -> Vec<String> {
        let found = select(Member::root(document), self.route);
        found
            .iter()
            .flat_map(|member| member.unsupported(self.allowed))
            .collect()
    }
}

/// Every YAML tag of `document`, as the dotted paths of the tagged nodes.
#[must_use]
pub fn tags(document: &Value) -> Vec<String> {
    Member::root(document).everywhere(Member::tagged)
}

/// Every `<<` merge key `apply_merge` left in `document`, as dotted paths. Compose resolves
/// them (a merge inside a merged anchor), so what is left unread would slip past the rules.
#[must_use]
pub fn merge_keys(document: &Value) -> Vec<String> {
    Member::root(document).everywhere(Member::merges)
}

/// Every key of `document` the model does not understand, as dotted paths.
#[must_use]
pub fn unsupported_keys(document: &Value) -> Vec<String> {
    RULES
        .iter()
        .flat_map(|rule| rule.unsupported_in(document))
        .collect()
}
