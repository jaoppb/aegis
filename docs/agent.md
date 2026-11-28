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

Callable only by a node tagged `tag:aegis-controller` (see the ACL snippet in [tailscale-acl.md](tailscale-acl.md)). The body names the commit as 7 to 64 lower-case hex digits, and may name one stack with `"stack"` (a stack name, such as `{"commit":"3fe0623","stack":"web"}`; `aegis sync --stack` sends it through the controller):

```sh
curl -X POST -H 'content-type: application/json' -d '{"commit":"3fe0623"}' \
  http://100.64.0.7:7480/v1/reconcile
```

The answer is the status report: the host, the commit and one entry per stack with its state (`running`, `failed`, `invalid`, `orphaned` or `purged`) and details (the failure, the violations of an invalid stack, which is never deployed, or what a removal kept):

```json
{"host":"jao-server","commit":"3fe0623","stacks":[{"directory":"hosts/jao-server/pihole","state":"running","details":[]}]}
```

| Status | Meaning |
|--------|---------|
| `200` | The reconcile ran; each stack's state is in the body. One failing stack does not stop the others. |
| `400` | The commit is not 7 to 64 lower-case hex digits, or the stack is not a valid stack name. A body that is not JSON with a `commit` string gets the framework's `415` or `422`. |
| `403` | The caller is not tagged `tag:aegis-controller`. |
| `502` | The checkout could not be moved to the commit; nothing was deployed. The body names the failure. |
| `503` | The whois lookup failed. |

Only one reconcile runs at a time; a second request waits for the first. A reconcile of one stack deploys, reports and removes that stack alone (no orphan sweep) and answers a report that lists that stack only; the agent does not post it to the controller, because it would stand for the whole host.

The agent also posts the same report to the controller's `POST /v1/status` ([controller.md](controller.md)). The post is best effort: a failure is ignored, and the answer above is the report either way.

## Removing stacks

Removal is declarative and never deletes data on its own.

- **A stack directory deleted from git.** The agent finds the stacks it used to run from Docker labels, so nothing is stored and a restart forgets nothing: a compose project named `aegis-*` that has a container started from under `<AEGIS_CHECKOUT>/hosts/<AEGIS_HOST>/`, or that keeps labelled volumes, and whose stack the repository no longer declares. It runs `docker compose -p aegis-<stack> down` (containers and networks, never `-v`) and reports the stack as `orphaned`, listing the volumes it kept. Reserved `aegis/` stacks, the agent's own container (`HOSTNAME`) and stacks that fail validation are never taken down, and nothing is taken down when the `hosts/` directory itself cannot be read. Host paths a removed stack mounted are kept but not listed, because its compose file is gone.
- **`state = "purged"` in `aegis.toml`.** The tombstone stays in git. The agent, in order: refuses unsafe paths before touching anything, runs `compose down`, asks the controller to delete the tailnet node (`DELETE /v1/nodes/{hostname}`, the exposed service's name, skipped when another live stack exposes the same name, or while any stack of the repository is rejected, because it may still expose that name; the controller deletes only a device whose tags are all in `AEGIS_ENROLL_TAGS`; a `404` that says `no such node` counts as already gone, any other `404` and a `409` for an ambiguous hostname fail), deletes the named volumes labelled `com.docker.compose.project=aegis-<stack>` (never `external` volumes or another project's), then deletes the stack's read-write absolute host paths. Any failure stops the chain and reports `failed`; the tombstone is still in git, so the next reconcile retries. When it finishes the stack reports `purged` ("purged, safe to delete dir"); delete the directory in a later commit. Only a user stack can be purged, and a tombstone on a stack that fails validation does nothing.
- **Host path safety.** A path is deleted only if it has at least three segments, is plain (no `.`, empty segment or `:`), is not the checkout or inside or above it, and is not inside or above a path another stack of the repository mounts. Host paths are not deleted while a stack of the host, or of the reserved `aegis/` directory, is rejected, because its mounts are unknown. The delete runs in a throw-away container that mounts the host's root, so it also reaches files a rootless daemon owns, and it refuses a symlink in any segment of the path, so it never follows a link out; a path that is already gone counts as deleted, and anything that is not a directory (a socket, a device, a file) is left alone. System locations such as `/etc`, `/dev`, `/run`, `/var/run` and `/var/lib/docker` are never deleted.
- **Known limits.** The path guard knows the paths the repository declares, so it cannot see the host paths a removed (orphaned) stack kept, the `driver_opts` bind devices of named volumes, a named volume another stack uses as `external`, or a filesystem mounted under a purged path (the delete crosses it). Keep such data out of a purged stack's volumes and paths. A user stack named like a reserved `aegis/` stack is never purged (rename it), and no stack is purged while the `aegis/` directory cannot be read; both report `failed` with the reason and are retried. The controller deletes any node whose tags are all in `AEGIS_ENROLL_TAGS` for any agent that asks; tying the delete to the purged stack's hostname is part of #76.
- **Reporting.** `orphaned` and `purged` are stack states in the status report, carried like `running`; `aegis status` and `aegis diff` ([cli.md](cli.md)) show them.

The controller needs the `devices:core` scope on its Tailscale OAuth client for the node delete.
