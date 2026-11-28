# Aegis

GitOps for Docker Compose stacks.

To install Aegis on a host, see [docs/bootstrap.md](docs/bootstrap.md). To check a repository, see what the hosts run and make them converge, see [docs/cli.md](docs/cli.md).

The controller and agents authenticate each other by tailnet identity: see [docs/tailscale-acl.md](docs/tailscale-acl.md) for the ACL policy and `AEGIS_LISTEN`, [docs/agent.md](docs/agent.md) for running the agent and its reconcile endpoint, [docs/controller.md](docs/controller.md) for running the controller and its status endpoint, [docs/sops.md](docs/sops.md) for encrypting secrets per host, [docs/exposure.md](docs/exposure.md) for exposing a service on the tailnet with HTTPS, and [docs/migrating-from-compose.md](docs/migrating-from-compose.md) for moving hand-written Compose stacks into the repository.
