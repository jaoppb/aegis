# Exposing a service on the tailnet

A stack whose `aegis.toml` has `expose = { service = "vaultwarden", port = 80 }` and `tailnet_tags = ["tag:app"]` gets a tailscale sidecar and Caddy from Aegis. Its `compose.yaml` describes only the app.

The agent adds a compose override (merged after the stack's own `compose.yaml`) with:

- a `tailscale` service: hostname and `TS_HOSTNAME` are the exposed service's name (a DNS label, so `vaultwarden` is served at `vaultwarden.<tailnet>.ts.net`), the node identity lives in the named volume `tailscale-state` (`aegis-<stack>_tailscale-state`), and the socket volume `tailscale-socket` is shared with Caddy. It needs `/dev/net/tun` and `NET_ADMIN`; #70 revisits both.
- a `caddy` service with `network_mode: service:tailscale`, the socket mounted read-only and a generated Caddyfile (an inline compose `config`, which needs Docker Compose 2.24.4 or later, as for the rest of the generated overrides): `<hostname>.<tailnet>.ts.net { reverse_proxy <service>:<port> }`. Caddy fetches the ts.net certificate through the socket.

A stack that exposes a service may not define services called `tailscale` or `caddy`, and the exposed service must exist in its compose file; otherwise the stack is rejected as invalid.

## How the node is enrolled

For each exposed stack the agent looks into the state volume with `docker run` (a throw-away container of the sidecar image, no network, volume read-only) and calls the controller's `POST /v1/enrollments` with the stack, the hostname, the tags and whether the volume is `empty` or `populated`. The controller answers with the tailnet's MagicDNS name (discovered through the Tailscale API, never configured) and, only when the volume is empty, a freshly minted single-use key. The agent hands the key to Compose in the `AEGIS_TS_AUTHKEY` environment variable and never writes it to a file, so a redeploy keeps the same node and hostname and mints nothing. The override is written to the tmpfs workspace and removed after `docker compose up`. If the state check, the controller or the render fails, only that stack fails.

## Controller settings

| Variable | Meaning |
|----------|---------|
| `AEGIS_TS_CLIENT_ID`, `AEGIS_TS_CLIENT_SECRET` | The Tailscale OAuth client, with the `auth_keys` and `devices:core` scopes. The secret comes decrypted from the deployment that runs the controller. |
| `AEGIS_ENROLL_TAGS` | Comma-separated ACL tags a stack's node may carry, such as `tag:app`. The controller refuses any other tag, and always refuses `tag:aegis-controller` and `tag:aegis-agent`. |

`POST /v1/enrollments` takes `{"stack":"vault","hostname":"vaultwarden","tags":["tag:app"],"state":"empty"}` and answers `{"tailnet":"taildfab0.ts.net","key":"tskey-..."}` (`key` is `null` when none was minted). It is served to `tag:aegis-agent` peers only, and only to the agent of a host of `AEGIS_AGENTS` ([controller.md](controller.md)). It answers `200` (nothing is stored, so there is no `201`), `400` for a malformed request, `403` for a caller that is no agent of the host or a request the desired state or the tag ceiling refuses, `429` with `Retry-After` over a key limit, `502` when Tailscale cannot be reached or refuses, and `503` before a commit has loaded.

The controller mints only for a stack the desired state places on the calling host: the stack must exist under `hosts/<host>/`, not be `purged` and declare `expose`, the hostname must be the exposed service's name, and the tags must be exactly the stack's `tailnet_tags` (an extra or a missing tag is refused). `AEGIS_ENROLL_TAGS` stays a ceiling on top. Until a commit has loaded, the controller answers `503`: it cannot tell yet, and that is not a refusal.

Keys are limited: 3 per stack of a host, 20 per host and 50 in total over one hour, counted once a request that would mint (`"state":"empty"`) passes the desired-state check, even if the tag ceiling or Tailscale then refuses it. Change them with `AEGIS_MINT_PER_STACK`, `AEGIS_MINT_PER_HOST`, `AEGIS_MINT_TOTAL` and `AEGIS_MINT_WINDOW_SECS` ([controller.md](controller.md)). The counts live in memory and start again when the controller restarts. A refusal for the desired state or for the key limit shows in `GET /v1/fleet` as `refusals`.

| Status | Meaning |
|--------|---------|
| `200` | The answer above. |
| `400` | An invalid stack, hostname or tag. |
| `403` | Another peer, a caller that is no agent of a host, a stack or hostname the desired state does not give the host, tags that are not the stack's, or a tag that is not allowed. |
| `429` | The key limit is reached; `Retry-After` says how many seconds until the oldest counted key leaves the window. |
| `502` | Tailscale fails. |
| `503` | The whois lookup failed, or no commit has loaded yet. |

A body that is not this JSON, or has no `stack`, gets the framework's `415` or `422`; an empty `stack` is a `400`. Error bodies are a plain-text reason that holds no secret. An agent older than the controller sends no `stack` and is refused (`422`), and an agent newer than the controller is refused by the old controller for the extra field (`422`): upgrade the controller and the agents together.

## Things to know

- The state check starts a throw-away container that mounts the volume, so Docker creates it without Compose's labels and Compose prints a warning about it on the first deploy. It is harmless.
- A deploy that fails before the node joins leaves the volume empty, so the next deploy mints another key; the unused single-use key expires after an hour.
- The auth key stays in the sidecar container's environment (`docker inspect`) until the container is recreated; it is single-use, so it cannot enrol a second node.
- The sidecar and Caddy images are pinned by tag, not by digest.
- A stack that exposes a service may not declare the volumes `tailscale-state` or `tailscale-socket`, which Compose would merge into the injected ones.
