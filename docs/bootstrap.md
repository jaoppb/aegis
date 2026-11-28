# Bootstrap

`aegis bootstrap` installs Aegis on a host once. It generates the host's age key, joins the host to the
tailnet, and starts the agent (and, with `--with-controller`, the controller) as compose stacks from the
GHCR images. After that, upgrades are git commits.

## Before you run it

The host needs `docker` with the compose plugin, `git`, `age` (for `age-keygen`) and `sops` 3.9 or later on
the `PATH`, and a user that can reach the Docker socket. You also need a Tailscale OAuth client that can
mint auth keys (`auth_keys` scope) and delete devices (`devices` scope), whose tags own
`tag:aegis-agent` and `tag:aegis-controller`. `docs/tailscale-acl.md` has the ACL snippet.

The OAuth client id and secret are read from the environment, never from arguments, so they stay out of
the process list:

```sh
export TS_OAUTH_CLIENT_ID=...
export TS_OAUTH_CLIENT_SECRET=...
aegis bootstrap --repo git@github.com:me/infra.git --host jao-server \
  --controller http://jao-server-aegis-controller:7480 --with-controller
```

| Flag | Meaning | Default |
|------|---------|---------|
| `--repo` | The git remote of the Aegis repository. | required |
| `--host` | The host's name: the directory under `hosts/`. | required |
| `--controller` | The URL the agent reports to. | required |
| `--with-controller` | Also install the controller on this host. | agent only |
| `--checkout` | Where the repository is cloned (`AEGIS_CHECKOUT` of the agent). | `/var/lib/aegis/repo` |
| `--stacks` | Where the `aegis/` stack files are rendered. | `/var/lib/aegis/stacks` |
| `--image-tag` | The tag of the `ghcr.io/jaoppb/aegis-agent` and `aegis-controller` images. | the version of `aegis` |

## What it does

1. Generates the age key at `$XDG_CONFIG_HOME/sops/age/keys.txt` (else `~/.config/sops/age/keys.txt`,
   the file `sops` reads by default; mode `0600`) with `age-keygen`, unless one exists, and prints its
   recipient. Put that recipient in `.sops.yaml` for `hosts/<host>/**`.
2. Clones the repository read-only into `--checkout`, or fetches it when it is already there. Bootstrap
   never pushes.
3. Renders `aegis/agent/compose.yaml` (and `aegis/controller/` with `--with-controller`) below
   `--stacks`: a Tailscale sidecar with a named state volume, and the Aegis container sharing its network.
4. Mints a single-use, pre-authorized, tagged key for each sidecar whose state volume is empty, and runs
   `docker compose up -d` for each stack. The controller starts first so the agent can report to it.
5. Sets `AEGIS_RESERVED` on the agent to the stacks it installed, so the agent keeps deploying them from
   the repository.

Copy the rendered `aegis/` directory into your repository and commit it. Aegis never commits for you.
Until then the agent has nothing to reconcile for `aegis/`.

Rerunning it is a no-op: the age key and the encrypted credentials are reused, no key is minted while a
sidecar's state volume holds a node, and `docker compose up -d` changes nothing that did not change.
A rerun still needs the OAuth variables.

## Secrets

The controller's long-lived OAuth credentials are written to `aegis/controller/controller.env`, encrypted
with `sops` to the host's age key, and the controller reads them through an `env_file`, which the agent
decrypts onto tmpfs exactly like any other stack (`docs/sops.md`). They are never put in a compose
environment, because Docker keeps a container's environment in plaintext on disk and `docker inspect`
shows it. Bootstrap writes the file only when it does not exist yet, so editing it later is yours.

The single-use `TS_AUTHKEY` is the exception: it reaches `docker compose` through its process
environment and the sidecar reads it from there. It is consumed when the node first joins and is useless
afterwards, but `docker inspect` of the sidecar container still shows it until the container is
recreated. Nothing writes it to a file, a log or an error.

## Limits

The agent also reconciles its own `aegis/agent` stack once it is committed. If that stack changes (a new
image tag), the agent recreates itself in the middle of a reconcile. The safe handoff is #26 (v0.2.0):
until then, upgrade the agent with `aegis bootstrap --image-tag <tag>` on the host. The `aegis/controller`
stack stays on the OAuth variable names `TS_OAUTH_CLIENT_ID` and `TS_OAUTH_CLIENT_SECRET`, which the
controller reads when it mints keys (#14).
