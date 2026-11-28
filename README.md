# Aegis

GitOps for Docker Compose stacks.

To install Aegis on a host, see [docs/bootstrap.md](docs/bootstrap.md).

The controller and agents authenticate each other by tailnet identity: see [docs/tailscale-acl.md](docs/tailscale-acl.md) for the ACL policy and `AEGIS_LISTEN`, [docs/agent.md](docs/agent.md) for running the agent and its reconcile endpoint, [docs/sops.md](docs/sops.md) for encrypting secrets per host, and [docs/exposure.md](docs/exposure.md) for exposing a service on the tailnet with HTTPS.
