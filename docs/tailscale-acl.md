# Tailscale ACL for the controller-agent API

The controller and the agents talk over the tailnet, and the tailnet is the perimeter. Every request is checked twice: the ACL below decides which nodes can reach each other at all, and each server then resolves the caller with `tailscale whois --json` and accepts it only when it carries the expected tag.

| Server | Accepts requests from | Listens on |
|--------|-----------------------|------------|
| Agent | nodes tagged `tag:aegis-controller` | `AEGIS_LISTEN` |
| Controller | nodes tagged `tag:aegis-agent` for every route except the sync routes, each but `GET /v1/health` bound to the host the node serves (its node name must be `<host>` or `<host>-aegis-agent` for a host of `AEGIS_AGENTS`, see [controller.md](controller.md)); an operator's untagged node for `GET /v1/fleet` and `GET /v1/health`; operators alone for the sync routes | `AEGIS_LISTEN` |

An untagged node, a node with another tag (including an agent calling another agent), an address that is not a tailnet node, or a failed whois lookup is refused, except an operator's untagged node on the controller routes open to operators (see Operators). The first three get `403`; a failed lookup gets `503`. The API is served under `/v1`.

## Operators

A person who runs `aegis status`, `diff` or `sync` ([cli.md](cli.md)) does it from a personal device, which stays untagged: tagging it would turn it into a shared device. The controller instead resolves the caller's login (`UserProfile.LoginName` of `tailscale whois --json`) and accepts an untagged node whose owner is listed in the controller's `AEGIS_OPERATORS`. A tagged node never counts as an operator, whatever login it reports. Operators reach only `GET /v1/fleet`, `GET /v1/health` and the sync routes; every other route stays for `tag:aegis-agent`, and the sync routes are for operators alone (an agent gets `403`). Logins are ASCII and compare without regard to case. Without `AEGIS_OPERATORS` no operator is admitted.

The ACL has to let the device reach the controller as well. Add a rule for the operators' own devices to the snippet below, for example `{"action": "accept", "src": ["ada@example.com"], "dst": ["tag:aegis-controller:7480"]}`.

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

Nodes get their tag from the auth key they join with. `aegis bootstrap` ([docs/bootstrap.md](bootstrap.md)) mints those keys, so create its OAuth client with the `auth_keys` (write) scope and select both `tag:aegis-controller` and `tag:aegis-agent` as its tags (with `--with-controller`, also the `devices:core` scope and every tag you pass to `--enroll-tags`, each with its own `tagOwners` entry): Tailscale mints a tagged key only for tags the OAuth client has. `autogroup:admin` in `tagOwners` lets an admin own the tags; it does not by itself let the OAuth client mint them. For a node you join by hand, use `tailscale up --advertise-tags=tag:aegis-agent`. In the containers bootstrap installs, each daemon shares its Tailscale sidecar's network and listens on `0.0.0.0:7480`. With kernel networking that port is also reachable on the Docker network of the stack, so what refuses a caller that is not a tagged tailnet node is the whois check above, not the listen address.

## Running a server

Both binaries read `AEGIS_LISTEN` and fail at start-up when it is missing or is not a socket address. The agent needs more variables, listed in [agent.md](agent.md), and the controller needs the repository, work directory and Tailscale variables listed in [controller.md](controller.md) (it fails at start-up without them):

```sh
AEGIS_LISTEN=100.64.0.7:7480 aegis-agent
```

Bind to the node's tailnet address, not `0.0.0.0` (the containers `aegis bootstrap` installs are the exception described above; whois still guards them); nothing checks the address, so a wildcard or LAN address exposes the plain-HTTP port beyond the tailnet. The transport is plain HTTP inside the WireGuard tunnel, and a peer outside the tailnet is not a tailnet node, so whois finds no match and it gets `403`. Do not share an Aegis node with another tailnet. The `tailscale` CLI must be on the `PATH` and the Tailscale daemon must be running on the host.

## Checking it

`GET /v1/health` answers `{"version":"v1","role":"agent"}` (or `controller`) to an allowed caller. Run the check from a node that carries the other side's tag (the controller node for an agent); an untagged machine gets `403` (on a controller, an operator's untagged node is answered too), and `503` means the whois lookup failed (the `tailscale` CLI is missing or the daemon is not reachable):

```sh
curl http://100.64.0.7:7480/v1/health
```
