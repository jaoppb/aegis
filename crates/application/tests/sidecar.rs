//! The sidecar override and Caddyfile rendered for a stack that exposes a service.

use aegis_application::{Exposure, KeyUse, SidecarSpec, state_volume_of};
use aegis_domain::{
    Expose, Node, Port, ProjectName, ServiceName, StackName, TailnetName, TailnetTag, TailnetTags,
};

#[cfg(test)]
fn exposure(service: &str) -> Exposure {
    let expose = Expose::new(ServiceName::new(service).unwrap(), Port::new(80).unwrap());
    let tags: TailnetTags = std::iter::once(TailnetTag::new("tag:app").unwrap()).collect();
    Exposure::new(expose, tags)
}

#[cfg(test)]
fn spec() -> SidecarSpec {
    let tailnet = TailnetName::new("taildfab0.ts.net").unwrap();
    SidecarSpec::new(&exposure("vaultwarden"), tailnet).unwrap()
}

#[test]
fn the_caddyfile_proxies_the_node_address_to_the_service() {
    let caddyfile = spec().caddyfile().unwrap();
    assert_eq!(
        caddyfile.as_str(),
        "vaultwarden.taildfab0.ts.net {\n  reverse_proxy vaultwarden:80\n}\n"
    );
}

#[test]
fn the_override_names_the_node_after_the_service_and_keeps_its_state_in_a_volume() {
    let text = spec().render(KeyUse::Fresh);
    let yaml = text.compose().as_str();
    assert!(yaml.contains("TS_HOSTNAME: vaultwarden\n"), "{yaml}");
    assert!(yaml.contains("- tailscale-state:/var/lib/tailscale\n"));
    assert!(yaml.contains("network_mode: service:tailscale\n"));
    assert!(yaml.contains("- tailscale-socket:/var/run/tailscale:ro\n"));
    assert!(yaml.contains("vaultwarden.taildfab0.ts.net {"));
}

#[test]
fn a_fresh_node_reads_its_key_from_the_environment_and_a_known_node_gets_none() {
    let fresh = spec().render(KeyUse::Fresh);
    let reused = spec().render(KeyUse::Reused);
    assert!(
        fresh
            .compose()
            .as_str()
            .contains("TS_AUTHKEY: \"${AEGIS_TS_AUTHKEY}\"")
    );
    assert!(!reused.compose().as_str().contains("TS_AUTHKEY"));
}

#[test]
fn the_override_is_plain_yaml_without_tags_or_anchors() {
    let rendered = spec().render(KeyUse::Fresh);
    let yaml = rendered.compose().as_str();
    assert!(
        !yaml.contains('!') && !yaml.contains('&') && !yaml.contains('*'),
        "{yaml}"
    );
}

#[test]
fn rendering_twice_gives_the_same_text() {
    assert_eq!(spec().render(KeyUse::Reused), spec().render(KeyUse::Reused));
}

#[test]
fn a_service_name_that_is_not_a_hostname_cannot_be_exposed() {
    let tailnet = TailnetName::new("t.ts.net").unwrap();
    assert!(SidecarSpec::new(&exposure("Web_App"), tailnet).is_err());
}

#[test]
fn the_state_volume_belongs_to_the_compose_project() {
    let project = ProjectName::of(&StackName::new("vaultwarden").unwrap()).unwrap();
    assert_eq!(
        state_volume_of(&project),
        "aegis-vaultwarden_tailscale-state"
    );
}

#[cfg(test)]
fn raw_spec() -> SidecarSpec {
    let tags: TailnetTags = std::iter::once(TailnetTag::new("tag:app").unwrap()).collect();
    let node = Node::new(ServiceName::new("sharefile").unwrap());
    let tailnet = TailnetName::new("taildfab0.ts.net").unwrap();
    SidecarSpec::new(&Exposure::raw(node, tags), tailnet).unwrap()
}

#[test]
fn a_raw_node_runs_no_caddy_and_has_no_caddyfile() {
    let rendered = raw_spec().render(KeyUse::Fresh);
    let yaml = rendered.compose().as_str();
    assert_eq!(rendered.caddyfile(), None);
    assert_eq!(raw_spec().caddyfile(), None);
    assert!(!yaml.contains("caddy"), "{yaml}");
    assert!(!yaml.contains("tailscale-socket"), "{yaml}");
    assert!(!yaml.contains("configs"), "{yaml}");
}

#[test]
fn a_raw_node_keeps_the_hostname_of_its_service_and_its_state_in_a_volume() {
    let yaml = raw_spec().render(KeyUse::Reused);
    let yaml = yaml.compose().as_str();
    assert!(yaml.contains("TS_HOSTNAME: sharefile\n"), "{yaml}");
    assert!(yaml.contains("hostname: sharefile\n"), "{yaml}");
    assert!(yaml.contains("- tailscale-state:/var/lib/tailscale\n"));
    assert!(yaml.contains("/dev/net/tun:/dev/net/tun"));
    assert!(!yaml.contains("TS_AUTHKEY"));
}

#[test]
fn a_raw_node_puts_its_service_in_the_network_namespace_of_the_sidecar() {
    let yaml = raw_spec().render(KeyUse::Fresh);
    let yaml = yaml.compose().as_str();
    let joined =
        "  sharefile:\n    network_mode: service:tailscale\n    depends_on:\n      - tailscale\n";
    assert!(yaml.contains(joined), "{yaml}");
    assert!(yaml.contains("TS_AUTHKEY: \"${AEGIS_TS_AUTHKEY}\""));
}

#[test]
fn the_raw_override_is_plain_yaml_with_only_the_state_volume() {
    let yaml = raw_spec().render(KeyUse::Fresh);
    let yaml = yaml.compose().as_str();
    assert!(yaml.ends_with("volumes:\n  tailscale-state:\n"), "{yaml}");
    assert!(!yaml.contains('!') && !yaml.contains('&') && !yaml.contains('*'));
}

#[test]
fn the_exposure_of_a_policy_with_a_node_is_raw() {
    let node = Node::new(ServiceName::new("sharefile").unwrap());
    let policy = aegis_domain::StackPolicy::default().with_node(node.clone());
    let exposure = Exposure::of(&policy).unwrap();
    assert_eq!(exposure.serving(), &aegis_application::Serving::Raw(node));
    assert_eq!(exposure.node().unwrap().as_str(), "sharefile");
}
