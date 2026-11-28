# Tailscale ACL for the controller-agent API

The controller and the agents talk over the tailnet, and the tailnet is the perimeter. Every request is checked twice: the ACL below decides which nodes can reach each other at all, and each server then resolves the caller with `tailscale whois --json` and accepts it only when it carries the expected tag.

| Server | Accepts requests from | Listens on |
|--------|-----------------------|------------|
| Agent | nodes tagged `tag:aegis-controller` | `AEGIS_LISTEN` |
| Controller | nodes tagged `tag:aegis-agent` | `AEGIS_LISTEN` |

An untagged node, a node with another tag (including an agent calling another agent), an address that is not a tailnet node, or a failed whois lookup is refused. The first three get `403`; a failed lookup gets `503`. The API is served under `/v1`.

## Policy snippet

Add this to the tailnet policy file (Access controls in the admin console). The ACL only restricts when no broader rule matches, so remove or narrow any allow-all rule such as `{"src": ["*"], "dst": ["*:*"]}`. Adjust the port to the one you set in `AEGIS_LISTEN`; `7480` is only an example.

```json
{
  "tagOwners": {
    "tag:aegis-controller": ["autogroup:admin"],
    "tag:aegis-agent": ["autogroup:admin"]
  },
  "acls": [
    {
      "action": "accept",
      "src": ["tag:aegis-controller"],
      "dst": ["tag:aegis-agent:7480"]
    },
    {
      "action": "accept",
      "src": ["tag:aegis-agent"],
      "dst": ["tag:aegis-controller:7480"]
    }
  ]
}
```

Tag the nodes when they join the tailnet, for example `tailscale up --advertise-tags=tag:aegis-agent`.

## Running a server

Both binaries read `AEGIS_LISTEN` and fail at start-up when it is missing or is not a socket address. The agent needs more variables, listed in [agent.md](agent.md), and the controller needs `AEGIS_TS_CLIENT_ID`, `AEGIS_TS_CLIENT_SECRET` and `AEGIS_ENROLL_TAGS` for its enrolment endpoint, listed in [exposure.md](exposure.md) (it fails at start-up without them):

```sh
AEGIS_LISTEN=100.64.0.7:7480 aegis-agent
```

Bind to the node's tailnet address, not `0.0.0.0`; nothing checks the address, so a wildcard or LAN address exposes the plain-HTTP port beyond the tailnet. The transport is plain HTTP inside the WireGuard tunnel, and a peer outside the tailnet is not a tailnet node, so whois finds no match and it gets `403`. Do not share an Aegis node with another tailnet. The `tailscale` CLI must be on the `PATH` and the Tailscale daemon must be running on the host.

## Checking it

`GET /v1/health` answers `{"version":"v1","role":"agent"}` (or `controller`) to an allowed caller. Run the check from a node that carries the other side's tag (the controller node for an agent); an untagged machine gets `403`, and `503` means the whois lookup failed (the `tailscale` CLI is missing or the daemon is not reachable):

```sh
curl http://100.64.0.7:7480/v1/health
```
