# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

The v0.1.0 interface is a CLI. The web dashboard (a separate TypeScript app) is planned and is the surface this record serves for design work.

## Stack

- **Core:** Rust, domain-driven (Cargo workspace, domain / application / adapters layers), with boundaries enforced by `arch-lint` as in `jaoppb/tools`.
- **Adapters (v0.1.0):** shell out to the `docker compose`, `sops` and `tailscale` CLIs behind ports. Minting Tailscale keys uses the Tailscale HTTP API, because the CLI cannot mint keys. Later these become Go FFI adapters or native Rust subsets, with no change to the domain.
- **UI:** a separate TypeScript web app talking to the controller's HTTP API. It is not part of the MVP, but the API boundary exists from day one.
- **Toolchain:** Nix flake devShell, `just`, Lefthook, GitHub Actions, issue and repository rules from `jaoppb/tools`.

## Users

A self-hoster running Docker Compose stacks on one or more personal hosts, starting with the author's own `jao-server`. They want the stacks, their secrets and their tailnet exposure to live in one git repository and to converge on their own, instead of SSH-ing in and editing `compose.yml` and `.env` files by hand.

## Product Purpose

Aegis is a GitOps container manager for Compose. A git repository is the single source of truth. Aegis pulls it, decrypts secrets, exposes services on the tailnet with HTTPS, keeps images current, and reconciles every host to match it.

Success for v0.1.0: every stack on `jao-server` runs from the repo through Aegis, and no plaintext `.env` file remains on the host.

## Positioning

Komodo, Portainer GitOps, doco-cd and Dockge each cover a slice. Aegis treats **Tailscale as the network and identity plane**:

- Every exposed service gets its own tailnet node, with a key Aegis mints from an OAuth client.
- HTTPS comes from ts.net certificates.
- The controller and agents trust each other through tailnet identity.
- Secrets are SOPS files that only the target host can decrypt.

The tailscale sidecar and Caddy boilerplate that each stack used to hand-write is generated.

## Operating Context

- **Repo layout:** `hosts/<host>/<stack>/compose.yaml`, plus an optional `aegis.toml` per stack for exposure, update policy and lifecycle.
- **Topology:** one controller handles git sync, Tailscale OAuth, image updates and the API. A thin agent per host drives the local Docker socket, including rootless sockets under `/run/user/<uid>/docker.sock`.
- **Transport:** controller and agents talk over the tailnet. ACL tags `tag:aegis-controller` and `tag:aegis-agent` limit who can connect, and each request is checked with Tailscale WhoIs.
- **Incumbent setup:** `jao-server:~/containers`, which is not in git. One directory per stack, each with a hand-written tailscale sidecar and Caddy on the shared tailscale socket. The tailnet name is hardcoded, `TS_AUTHKEY`/`TS_AUTH_KEY` naming is inconsistent, secrets sit in plaintext `.env` files, a `scripts/certificate.sh` copies raw certs out, and `sharefile` builds local images from Dockerfiles.

## Capabilities and Constraints

### Decisions

- **Users and runtime:** self-hosters running Docker Compose on 1 to N hosts. Git is the source of truth.
- **Topology:** controller plus agents. Running stacks survive a controller outage.
- **Secrets:** SOPS in the repo. **Each agent decrypts** with its own age key; `.sops.yaml` creation rules scope recipients per host. The controller never sees plaintext. Decrypted values go to tmpfs or env and are never written to disk in plaintext.
- **Image updates:** the controller detects new tags or digests and **commits the bump back to git**, either as a PR or with auto-merge depending on per-stack policy. The normal sync deploys it, so rollback is `git revert`. Services built from the repo are skipped.
- **Tailscale:** an OAuth client mints **single-use, pre-authorized, tagged, non-ephemeral** keys, only when a sidecar has no state yet. Node state lives in a named volume, so hostnames stay stable. Purge deletes the node through the API. Agents enroll the same way.
- **Sidecars:** **Aegis injects them.** `compose.yaml` describes only the app. `expose = { service, port }` in `aegis.toml` makes Aegis render a compose override with the tailscale sidecar and Caddy (generated Caddyfile, ts.net certificate through the socket). The rendered override can be viewed with `aegis diff`.
- **Certificates:** v0.1.0 uses ts.net HTTPS only. Public domains with ACME DNS-01 come later.
- **Mounts:** persistent data uses **named volumes or absolute host paths only**. A relative bind mount is valid only if the path exists in the repo and is mounted `:ro` (config files). Anything else fails validation at sync time.
- **Builds:** compose `build:` is supported. The agent builds from the checked-out repo and tags the image with the commit SHA.
- **Removal:** deleting a stack from git runs `compose down`. Volumes and data are kept and shown as orphaned. A full wipe is declarative: set `state = "purged"` in `aegis.toml`, a tombstone kept in git. Aegis then runs down, deletes the named volumes and declared absolute paths, and removes the tailnet node.
- **Bootstrap:** Aegis runs as containers. `aegis bootstrap` installs it once under a reserved `aegis/` directory in the repo, and from then on upgrades are git commits. The agent never recreates itself in the middle of a reconcile; it hands off to a detached process and rolls back on a failed health check. The safe handoff ships in v0.2.0.
- **Sync trigger:** v0.1.0 polls git. Webhooks come later.

### Non-goals

- Scheduling, placement or orchestration across hosts. Assigning a stack to a host is a directory, not a decision Aegis makes.
- Kubernetes, Swarm or Nomad support.
- Multi-user, roles or tenancy. One operator.
- Working without Tailscale. Aegis depends on Tailscale by design.
- Deleting data implicitly. Only the `purged` tombstone deletes volumes.
- Floating-tag, in-place updates in the style of Watchtower.
- In v0.1.0: the web UI, ACME/public domains, exporting raw cert files, image auto-update, a verified multi-host fleet, and webhooks.

### Open questions

- **Image update policy granularity:** per stack or per service, and is semver range syntax needed? It affects the `aegis.toml` schema in v0.2.0. Per-stack is simpler; per-service handles mixed stacks such as monitoring.
- **Controller state:** is it purely derived (git plus a live query to agents), or is there a local store for event history? Derived means nothing to back up; a store gives history for the UI.
- **Apps that need raw cert files** (clip-cascade today): should Aegis export certs into a volume, or should such apps sit behind the injected Caddy? Export keeps the app as it is; Caddy means less code.
- **Poll interval and per-host reconcile concurrency:** set the defaults during implementation.

### Risks

- The agent mounts the Docker socket, so a compromised agent or controller means root (or rootless-user) control of the host.
- Self-upgrade handoff: a bad agent release could stop reconciling, including the fix for itself. Needs a health-checked rollback.
- Migration: hand-made tailnet nodes must be deleted before Aegis mints new ones, or hostnames become `vaultwarden-1` and certs and bookmarks break.
- Committing back to git for updates gives the controller write access to the repo. A leaked token can deploy anything.
- Parsing CLI output in the v0.1.0 adapters is brittle across docker, sops and tailscale versions. The flake needs to pin them.
- Losing an age key makes that host's secrets unrecoverable unless a backup recipient is configured.

## Brand Commitments

Name: **Aegis**. Tagline: "GitOps-driven container manager".

## Evidence on Hand

No users, benchmarks or screenshots exist yet. The incumbent `jao-server` stacks (vaultwarden, monitoring, ferrous, clip-cascade, sharefile, proxy, pihole, ai-memory) are the real migration target and test corpus.

## Product Principles

1. **Git is the truth, and the host is disposable.** Anything that changes running state goes through a commit, including image bumps and purges.
2. **Never lose data by accident.** Deleting data is explicit, declarative and auditable.
3. **Plain Compose stays runnable.** A stack's `compose.yaml` stays a valid Compose file. Aegis adds overrides; it does not take over the format.
4. **Boundaries before features.** The domain never knows about a CLI. Adapters are replaceable, and arch-lint enforces this.
5. **The tailnet is the perimeter.** Identity, transport and HTTPS all come from Tailscale.
