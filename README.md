# Aegis

GitOps for Docker Compose stacks.

The controller and agents authenticate each other by tailnet identity: see [docs/tailscale-acl.md](docs/tailscale-acl.md) for the ACL policy and `AEGIS_LISTEN`, [docs/agent.md](docs/agent.md) for running the agent and its reconcile endpoint, [docs/controller.md](docs/controller.md) for running the controller and its status endpoint, and [docs/sops.md](docs/sops.md) for encrypting secrets per host.
