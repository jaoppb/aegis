# Migrating from hand-written Compose

This guide moves a directory of hand-written Compose stacks (one directory per stack, a `.env` next to each, a Tailscale sidecar and Caddy copied into every stack) into an Aegis GitOps repository. The worked example is the author's `jao-server:~/containers`; every step applies to any host.

Nothing here changes a host by itself: you edit files in a checkout, and the only commands that touch a host are the ones named in a step. Do one stack at a time, and finish "Verify" before you start the next.

## What changes

| Before | After |
|--------|-------|
| `~/containers/<stack>/docker-compose.yml` | `hosts/<host>/<stack>/compose.yaml` in git |
| Project name from the directory | Project `aegis-<stack>` ([agent.md](agent.md)) |
| A `tailscale` and a `caddy` service in every stack | `[expose]` in `aegis.toml`; Aegis injects both ([exposure.md](exposure.md)) |
| Plaintext `.env` on the host | `.env` encrypted with SOPS, decrypted onto tmpfs per deploy ([sops.md](sops.md)) |
| `TS_AUTHKEY` pasted by hand | Single-use key minted by the controller, only when the node's state volume is empty |
| `docker compose up -d` over SSH | A commit; the agent converges the host |

## Before you start

1. **Aegis is installed on the host.** Follow [bootstrap.md](bootstrap.md): `aegis bootstrap` generates the host's age key, joins the host to the tailnet and starts the agent and controller. Copy the rendered `aegis/` directory into the repository and commit it.
2. **The ACL is in place** ([tailscale-acl.md](tailscale-acl.md)), and `--enroll-tags` includes every tag your stacks will carry, such as `tag:app`.
3. **`.sops.yaml` names the host.** Put the recipient `aegis bootstrap` printed in a rule for `hosts/<host>/`, together with an offline backup recipient ([sops.md](sops.md)). Only ever paste public `age1...` recipients into the repository; the private key stays on the host and in your backup.
4. **You have a local checkout and `sops`**, so you can encrypt and check files on your machine; encrypting needs only the public recipients in `.sops.yaml`. The backup private key stays in offline storage and comes out only to decrypt and check a file. `aegis validate` runs offline.
5. **A rollback point.** Keep the old directory on the host untouched until the stack is verified.

## Per stack

### 1. Move the files

Create `hosts/<host>/<stack>/` and copy `compose.yaml` in. The stack name becomes the project `aegis-<stack>`, so use lower-case letters, digits, `-` and `_`. Aegis always reads `compose.yaml`, never `compose.override.yaml`, `compose.yml` or `docker-compose.yml`, so fold an override file into it.

Aegis checks each stack before it deploys it, and rejects what it cannot vouch for. Run `aegis validate` often; the usual rejections are keys outside the allowlist (`devices`, `volumes_from` and `label_file`, among others) and mounts, covered below.

### 2. Replace the hand-written sidecars with `expose`

Delete the `tailscale` and `caddy` services, their `tailscale-state` and `tailscale-socket` volumes, the `/dev/net/tun` device and the Caddyfile, and any `TS_AUTHKEY` variable. A stack that exposes a service may not define services called `tailscale` or `caddy`, nor those volumes. Then add `aegis.toml` next to `compose.yaml`:

```toml
tailnet_tags = ["tag:app"]

[expose]
service = "vaultwarden"
port = 80
```

`service` is the compose service that serves HTTP, and its name is the tailnet hostname: `vaultwarden` is served at `https://vaultwarden.<tailnet>.ts.net`. If the old node was called something else, rename the service (or pick the name you want) so the hostname stays what your bookmarks use. `tailnet_tags` must be tags the controller allows (`AEGIS_ENROLL_TAGS`) and are exactly the tags of the node. An app that listens on HTTPS itself or that needs raw certificate files is not covered: v0.1.0 serves ts.net HTTPS through the injected Caddy only (`PRODUCT.md`). The app must speak plain HTTP to Caddy, so turn off its own TLS.

Stacks with no `expose` (an internal service, a database, a DNS server) need no `aegis.toml`.

### 3. Put data in named volumes or absolute paths

Persistent data uses named volumes or absolute host paths only. A relative bind mount (`./config:/etc/app`) is valid only for a path that is in the repository and mounted `:ro`, which suits a config file and nothing else.

Names change when the project does. A named volume `data` of the old project `vaultwarden` is `vaultwarden_data`, and the same volume under Aegis is `aegis-vaultwarden_data`, which starts empty. Pick one:

- **Absolute host path.** Bind-mount the directory that already holds the data (`/srv/vaultwarden:/data`). Nothing is copied, and the path survives the project. A purge later deletes absolute paths the stack mounts read-write, so keep them dedicated to the stack.
- **Keep the old named volume.** Declare it `external: true` with its old full name. Aegis never purges an external volume, so this also means Aegis never cleans it up.
- **Copy into the new volume.** Stop the old stack, create `aegis-<stack>_<volume>` and copy the data into it with a throw-away container. Do this before the first reconcile, so the app never starts on an empty volume.

Whichever you choose, back the data up first. After a copy, compare the old and new contents (file counts and sizes, or start the app and look at the data) before you remove the old volume.

### 4. Encrypt the secrets

For each `.env`, `env_file` and `secrets` file of the stack, encrypt it in place in the checkout, from the repository root so `.sops.yaml` matches:

```sh
sops encrypt --in-place hosts/<host>/<stack>/.env
```

Then check each file before you stage it (a copied plaintext `.env` is one `git add -A` from a commit): `sops filestatus <file>` reports `"encrypted":true` (pass `--input-type dotenv` for a dotenv file not named `.env` or `*.env`), and `aegis validate` rejects any `.env`, `env_file` or `secrets` file that is not SOPS-encrypted. Only then `git add`, and read `git diff --cached` before you commit: it shows `ENC[...]` values and a `sops_` metadata block, never a plain secret. Mind the rules that Aegis checks ([sops.md](sops.md)):

- A file with another name than `.env` or `*.env` needs `--input-type dotenv --output-type dotenv`, or `sops` encrypts it as binary and the agent cannot read it.
- A PEM or another raw value goes into a YAML or JSON file, encrypted in that format.
- A dotenv file is ASCII only, has no variable starting with `sops_` in any case, and sets no `COMPOSE_*`, `DOCKER_*`, `PATH` or `HOME`.
- Keep multi-line values in a secret file, not in `.env`.
- Never commit the plaintext. If a plaintext secret reaches a commit, treat it as leaked and rotate it; deleting the file in a later commit does not remove it from history.

Copy secrets between machines over a channel you trust, and never paste them into an issue, a chat or a pull request. Once the stack is verified, revoke the old hand-made auth keys in the Tailscale admin console (Settings, Keys), because a reusable key left in an old `.env` can still enrol nodes, and remove the plaintext `.env` from the host (`shred -u` is best effort on journaling and copy-on-write filesystems, so rotating the secret is the sure way) and from your checkout.

### 5. Delete the hand-made tailnet node before Aegis mints its own

This is the step that breaks hostnames when it is skipped. Tailscale keeps node names unique, so if the old node `vaultwarden` still exists when Aegis enrols a new one, the new node becomes `vaultwarden-1`, and the certificate, the bookmarks and every ACL rule that names the host break.

For a stack with `expose`, in this order:

1. Stop the old stack so it does not reconnect and so its ports are free: `docker compose down` in the old directory, **without `-v`**, which would delete its volumes.
2. Delete the old node in the Tailscale admin console (Machines, the node, Delete). Check that the name is free before you go on.
3. Only then commit and let Aegis deploy. Its first reconcile finds an empty `tailscale-state` volume, asks the controller for a single-use key and joins as the clean hostname.

The old node is the only thing you delete by hand. Aegis never needs the old state volume, and later removal of the stack is `state = "purged"` (below), which deletes the new node through the API.

### 6. Commit and let Aegis deploy

Commit the stack with its encrypted files, one stack per commit and pull request, and push. The controller polls the repository; to deploy now, run `aegis sync --stack <stack>`. Do the cutover in one sitting: the old stack is already stopped, so the service is down from step 5 until the agent reports it `running`.

## Verify

```sh
aegis validate                 # offline; every stack valid
aegis diff                     # the stack is "same", and the override of an exposed stack looks right
aegis status                   # the host confirmed the target commit and the stack is running
```

Then check what a user sees: the service answers at its previous `https://<hostname>.<tailnet>.ts.net`, the hostname has no `-1` suffix in the admin console, the data is there, and `docker compose ls` shows `aegis-<stack>`. A stack that is `failed` reports the file and the reason (never a secret); fix it in git and push. A stack that exposes a service and has a node that is `<name>-1` means step 5 was skipped: delete the old node, delete the `-1` node, remove the volume `aegis-<stack>_tailscale-state` (the sidecar's state only, never a data volume), and let the next reconcile mint again.

## Rolling back

Until you delete it, the old directory is the way back. To return a stack to hand-run Compose: remove its directory from git (the agent runs `compose down` and keeps the volumes, shown as `orphaned`), then `docker compose up -d` in the old directory. Removing the directory does not delete the node Aegis minted, so delete it in the admin console before the old sidecar starts with a new auth key, or its hostname becomes `<name>-1`. Data in an absolute path or an external volume is shared by both, so stop one before starting the other.

## Removing a stack for good

Deleting the directory only stops the stack. To wipe it, set `state = "purged"` in its `aegis.toml`, commit, and delete the directory in a later commit once the agent reports it purged. Purging deletes the stack's named volumes, its read-write absolute host paths and its tailnet node. Its path guard has limits: it crosses a filesystem mounted under a purged path and cannot see `driver_opts` bind devices or a volume another stack uses as `external`, so keep mount points and shared data out of a purged stack's volumes and paths ([agent.md](agent.md), "Known limits"). Check the paths before you commit: this is the one operation that deletes data. For an exposed stack Aegis calls `DELETE /v1/nodes/{hostname}` on the controller, whose OAuth client needs the `devices:core` scope.

## Controller routes

The routes the migration touches are `POST /v1/enrollments` (a node's key, step 5) and `DELETE /v1/nodes/{hostname}` (a purge). [controller.md](controller.md) lists every route with its caller and statuses, and [agent.md](agent.md) the one the agent serves.

## What is not migrated

An app that needs raw certificate files (a `scripts/certificate.sh` that copies certificates out of Tailscale for an app to read) does not move: v0.1.0 has no certificate export. Put the app behind the injected Caddy, or retire it. The author retired the one such app (`clip-cascade`) and its `certificate.sh` instead of migrating them.
