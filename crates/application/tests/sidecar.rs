//! The sidecar override and Caddyfile rendered for a stack that exposes a service.

use aegis_application::{Exposure, KeyUse, SidecarSpec, state_volume_of};
use aegis_domain::{
    Expose, Port, ProjectName, ServiceName, StackName, TailnetName, TailnetTag, TailnetTags,
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
    let caddyfile = spec().caddyfile();
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
