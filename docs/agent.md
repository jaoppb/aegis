# Running the agent

The agent converges one host to a git commit. The controller tells it which commit with `POST /v1/reconcile`; the agent moves its own git checkout to that commit, loads and validates the stacks of its host, and brings each valid stack up with `docker compose`.

## Configuration

The agent reads its configuration from the environment and exits at start-up when a required variable is missing or invalid, or when it cannot remove the decrypted files an earlier run left in its secret directory (`$XDG_RUNTIME_DIR/aegis`, or `/dev/shm/aegis`).

| Variable | Required | Meaning |
|----------|----------|---------|
| `AEGIS_LISTEN` | yes | Socket address to listen on, normally the node's tailnet address (see [tailscale-acl.md](tailscale-acl.md)). |
| `AEGIS_HOST` | yes | The host this agent deploys: the directory name under `hosts/`. |
| `AEGIS_CHECKOUT` | yes | Absolute path of a git working tree of the repository, cloned beforehand with an `origin` remote. |
| `AEGIS_CONTROLLER` | yes | Base URL of the controller API, such as `http://100.64.0.2:7480`. |
| `AEGIS_RESERVED` | no | Comma-separated names of the reserved `aegis/` stacks this host deploys. Empty by default, so nothing under `aegis/` runs unless a host opts in. |

```sh
AEGIS_LISTEN=100.64.0.7:7480 AEGIS_HOST=jao-server AEGIS_CHECKOUT=/var/lib/aegis/repo \
  AEGIS_CONTROLLER=http://100.64.0.2:7480 aegis-agent
```

The checkout must already exist: the agent runs `git fetch origin` and `git checkout --detach <commit>` in it, so the user the agent runs as needs read access to the remote (an SSH agent socket in `SSH_AUTH_SOCK` or a credential helper reachable from `HOME`) and write access to the directory. git never prompts, so SSH host keys must already be trusted: mount a `HOME` (in the image, `/var/lib/aegis/home`) holding `.ssh/known_hosts`, or use an HTTPS remote with a credential helper. git inherits only `PATH`, `HOME`, `XDG_CONFIG_HOME`, `SSH_AUTH_SOCK`, `SSL_CERT_FILE` and `GIT_SSL_CAINFO`. In a container, the checkout path is a mounted volume owned by the agent's user, or git refuses it as a "dubious ownership" repository.

## Docker

The agent runs `docker compose` against the local daemon. The socket comes from `DOCKER_HOST` (also `DOCKER_CONTEXT`, `DOCKER_CONFIG`, `DOCKER_CERT_PATH` and `DOCKER_TLS_VERIFY`); apart from `PATH`, `HOME`, `XDG_RUNTIME_DIR` and `SSL_CERT_FILE`, no other variable of the agent reaches Compose, so `COMPOSE_FILE` or `COMPOSE_PROJECT_NAME` in its environment have no effect.

- Rootful: `DOCKER_HOST=unix:///var/run/docker.sock`.
- Rootless: `DOCKER_HOST=unix:///run/user/<uid>/docker.sock`.

In a container, mount that socket into the agent container.

Every stack runs as the project `aegis-<stack>` (so the stack name must be lower-case letters, digits, `-` and `_`; otherwise it is reported `failed`), always with `-f compose.yaml`: `compose.override.yaml`, `compose.yml` and `docker-compose.yml` in the stack directory are never read. Services with `build:` are built from the checkout and also tagged `aegis-<stack>-<service>:<commit>`.

## Secrets

Each stack's SOPS-encrypted `env_file`, `secrets` and `.env` files are decrypted by the agent with its own age key onto tmpfs for the length of `docker compose up`, and removed afterwards. `sops` (3.9 or later, for `sops decrypt`) must be on the `PATH`, and the agent needs its age key: set `SOPS_AGE_KEY_FILE` (or `SOPS_AGE_KEY_CMD`, or `SOPS_AGE_SSH_PRIVATE_KEY_FILE`), or place the key at the default path `$XDG_CONFIG_HOME/sops/age/keys.txt`. `sops` inherits only `PATH`, `HOME`, `XDG_CONFIG_HOME`, `XDG_RUNTIME_DIR` and those variables. Docker Compose 2.24.4 or later is required for the generated override (the release image pins compatible versions through `flake.lock`). A decryption failure marks only that stack `failed`.

A stack that exposes a service (`expose` in its `aegis.toml`) also needs the Docker daemon to run the sidecar images (`tailscale/tailscale`, `caddy`) and a throw-away container that checks the sidecar's state volume, and the agent calls the controller's `POST /v1/enroll`; see [exposure.md](exposure.md). See [sops.md](sops.md) for the tmpfs requirement, the container mount and the `.sops.yaml` rules.

## `POST /v1/reconcile`

Callable only by a node tagged `tag:aegis-controller` (see the ACL snippet in [tailscale-acl.md](tailscale-acl.md)). The body names the commit as 7 to 64 lower-case hex digits:

```sh
curl -X POST -H 'content-type: application/json' -d '{"commit":"3fe0623"}' \
  http://100.64.0.7:7480/v1/reconcile
```

The answer is the status report: the host, the commit and one entry per stack with its state (`running`, `failed` or `invalid`) and details (the failure, or the violations of an invalid stack, which is never deployed):

```json
{"host":"jao-server","commit":"3fe0623","stacks":[{"directory":"hosts/jao-server/pihole","state":"running","details":[]}]}
```

| Status | Meaning |
|--------|---------|
| `200` | The reconcile ran; each stack's state is in the body. One failing stack does not stop the others. |
| `400` | The commit is not 7 to 64 lower-case hex digits. A body that is not JSON with a `commit` string gets the framework's `415` or `422`. |
| `403` | The caller is not tagged `tag:aegis-controller`. |
| `502` | The checkout could not be moved to the commit; nothing was deployed. The body names the failure. |
| `503` | The whois lookup failed. |

Only one reconcile runs at a time; a second request waits for the first.

The agent also posts the same report to the controller's `POST /v1/status`. The controller does not serve that route yet (#14), so until then the post is best effort and the answer above is the report. Stacks with `state = "purged"` are skipped (#20).
