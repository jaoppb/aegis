# Bootstrap

`aegis bootstrap` installs Aegis on a host once. It generates the host's age key, joins the host to the
tailnet, and starts the agent (and, with `--with-controller`, the controller) as compose stacks from the
GHCR images. After that, upgrades are git commits.

## Before you run it

The host needs `docker` with the compose plugin, `git`, `age` (for `age-keygen`) and `sops` 3.9 or later on
the `PATH`. Run it as root, or as a user who can reach the Docker socket and owns `/var/lib/aegis` (or point
`--checkout` and `--stacks` at directories it owns). The age key lands in the home of that user:
`/root/.config/sops/age/keys.txt` for root. Under `sudo`, `HOME` may still be the caller's, so check where
the key ends up before you rely on the path.

You also need a Tailscale OAuth client with the `auth_keys` (write) scope and both `tag:aegis-agent` and
`tag:aegis-controller` selected as its tags, so it can mint keys for them (`docs/tailscale-acl.md` has the
ACL snippet and how the tags are owned). Every bootstrap needs it in the environment, including an
agent-only one, so it spreads that client's credentials across the hosts you bootstrap.

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
   recipient. Put that recipient in `.sops.yaml` for `hosts/<host>/**`. `aegis/controller/controller.env`
   is encrypted to this host's key alone: add a `creation_rules` entry for `aegis/**` (with a backup
   recipient, as `docs/sops.md` recommends) if you want it re-encrypted for more keys.
2. Clones the repository read-only into `--checkout`, or fetches it when it is already there. Bootstrap
   never pushes.
3. Renders `aegis/agent/compose.yaml` (and `aegis/controller/` with `--with-controller`) below
   `--stacks`: a Tailscale sidecar with a named state volume, and the Aegis container sharing its network.
4. Mints a single-use, pre-authorized, tagged key for each sidecar whose state volume is empty, and runs
   `docker compose up -d` for each stack. The controller starts first so the agent can report to it.
5. Sets `AEGIS_RESERVED` on the agent to the stacks it installed, so the agent keeps deploying them from
   the repository.

Copy the rendered `aegis/` directory into your repository and commit it. Aegis never commits for you.
Until then the agent reconciles no `aegis/` stack, and the `up` that bootstrap ran is all that keeps them
running. A `repo` clone that does not hold `aegis/` yet is fine.

Rerunning it creates nothing twice: the age key and the encrypted credentials are reused, no key is
minted while a sidecar's state volume holds a node, and the files are left alone when they have not
changed. The report says "nothing to do" in that case. Success means Compose started the containers: the
sidecar has a healthcheck and the agent and controller wait for it, but bootstrap does not wait for the
node to appear on the tailnet, so check `tailscale status` or `docker compose ps` afterwards.

The first keyless `up` after a minted one (a rerun, or the agent's first reconcile of the committed
`aegis/agent`) recreates the sidecar once, because its `TS_AUTHKEY` goes from the key to empty, and with it
the agent or controller that shares its network. The node keeps its identity through the state volume. If
`up` fails after a key was minted, that unused single-use key stays valid until it expires, and the rerun
mints another.

## Secrets

The controller's long-lived OAuth credentials are written to `aegis/controller/controller.env`, encrypted
with `sops` to the host's age key, and the controller reads them through an `env_file`, which the agent
decrypts onto tmpfs exactly like any other stack (`docs/sops.md`). So the plaintext is never in git or in a
file on disk. A running container's environment is another matter: Compose copies `env_file` values into
the container's environment, which Docker stores in its container config and `docker inspect` prints, so
anyone who can reach the Docker socket can read the credentials of the running controller. That is the
same exposure as any secret an Aegis stack gets through `env_file`, and it is why the credentials are not
put in `environment:` of the rendered compose file either.

The single-use `TS_AUTHKEY` reaches `docker compose` through its process environment and the sidecar reads
it from there. It is consumed when the node first joins and is useless afterwards, but `docker inspect` of
the sidecar container still shows it until the container is recreated. Nothing writes it to a file, a log
or an error.

The `--repo` URL must not carry a password (bootstrap refuses it): it would show in the process list and be
written to the checkout's git config. Use an SSH remote with a key, or a git credential helper. The
images are pinned by tag; `--image-tag` takes the tag you trust, and the agent holds the Docker socket, so
a re-pushed tag runs on the host.

## Limits

The agent also reconciles its own `aegis/agent` stack once it is committed. If that stack changes (a new
image tag), the agent recreates itself in the middle of a reconcile. The safe handoff is #26 (v0.2.0).
Until then, to upgrade, re-render with `aegis bootstrap --image-tag <tag>` on the host, copy the new
`aegis/` files into the repository and commit them: running bootstrap alone is not enough, because the
agent redeploys the committed files, which still name the old tag. The `aegis/controller` stack stays on
the OAuth variable names `TS_OAUTH_CLIENT_ID` and `TS_OAUTH_CLIENT_SECRET`, which the controller reads when
it mints keys (#14).
