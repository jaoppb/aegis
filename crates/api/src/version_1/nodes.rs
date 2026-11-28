//! The tailnet node resource: an agent asks the controller to delete the node of a purged stack
//! with `DELETE /v1/nodes/{hostname}`.

/// The route of a tailnet node, served by the controller. `{hostname}` is the node's DNS label.
///
/// `DELETE` removes the node: `204` when it was deleted, `404` when no such node exists (already
/// gone), `409` when several nodes share the hostname (nothing is deleted) and `502` when
/// Tailscale cannot be reached or refuses.
pub const NODE_PATH: &str = "/v1/nodes/{hostname}";

/// The body of the `404` that means the node does not exist. Any other `404` (a controller that
/// does not serve the route, or a proxy in between) is not an answer about the node.
pub const NODE_GONE: &str = "no such node";

/// The path of the node with `hostname`, a DNS label of lower-case letters, digits and `-`.
#[must_use]
pub fn node_path(hostname: &str) -> String {
    format!("/v1/nodes/{hostname}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_node_path_names_the_hostname_under_nodes() {
        assert_eq!(node_path("vaultwarden"), "/v1/nodes/vaultwarden");
    }
}
