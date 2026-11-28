# Exposing a service on the tailnet

A stack whose `aegis.toml` has `expose = { service = "vaultwarden", port = 80 }` and `tailnet_tags = ["tag:app"]` gets a tailscale sidecar and Caddy from Aegis. Its `compose.yaml` describes only the app.

The agent adds a compose override (merged after the stack's own `compose.yaml`) with:

- a `tailscale` service: hostname and `TS_HOSTNAME` are the exposed service's name (a DNS label, so `vaultwarden` is served at `vaultwarden.<tailnet>.ts.net`), the node identity lives in the named volume `tailscale-state` (`aegis-<stack>_tailscale-state`), and the socket volume `tailscale-socket` is shared with Caddy. It needs `/dev/net/tun` and `NET_ADMIN`; #70 revisits both.
- a `caddy` service with `network_mode: service:tailscale`, the socket mounted read-only and a generated Caddyfile (an inline compose `config`, which needs Docker Compose 2.23.1 or later): `<hostname>.<tailnet>.ts.net { reverse_proxy <service>:<port> }`. Caddy fetches the ts.net certificate through the socket.

A stack that exposes a service may not define services called `tailscale` or `caddy`, and the exposed service must exist in its compose file; otherwise the stack is rejected as invalid.

## How the node is enrolled

For each exposed stack the agent looks into the state volume with `docker run` (a throw-away container of the sidecar image, no network, volume read-only) and calls the controller's `POST /v1/enroll` with the hostname, the tags and whether the volume is `empty` or `populated`. The controller answers with the tailnet's MagicDNS name (discovered through the Tailscale API, never configured) and, only when the volume is empty, a freshly minted single-use key. The agent hands the key to Compose in the `AEGIS_TS_AUTHKEY` environment variable and never writes it to a file, so a redeploy keeps the same node and hostname and mints nothing. The override is written to the tmpfs workspace and removed after `docker compose up`. If the state check, the controller or the render fails, only that stack fails.

## Controller settings

| Variable | Meaning |
|----------|---------|
| `AEGIS_TS_CLIENT_ID`, `AEGIS_TS_CLIENT_SECRET` | The Tailscale OAuth client, with the `auth_keys` and `devices:core` scopes. The secret comes decrypted from the deployment that runs the controller. |
| `AEGIS_ENROLL_TAGS` | Comma-separated ACL tags a stack's node may carry, such as `tag:app`. The controller refuses any other tag, and always refuses `tag:aegis-controller` and `tag:aegis-agent`. |

`POST /v1/enroll` takes `{"hostname":"vaultwarden","tags":["tag:app"],"state":"empty"}` and answers `{"tailnet":"taildfab0.ts.net","key":"tskey-..."}` (`key` is `null` when none was minted). It is served to `tag:aegis-agent` peers only: `400` for an invalid hostname or tag, `403` for a tag that is not allowed, `502` when Tailscale fails.
