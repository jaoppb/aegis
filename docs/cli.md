# The `aegis` command line

`aegis` is the operator's interface. `validate` and `diff` read a checkout of the repository on your machine, `status`, `diff` and `sync` call the controller, and `bootstrap` installs Aegis on a host ([bootstrap.md](bootstrap.md)).

| Command | What it does |
|---------|--------------|
| `aegis validate [path]` | Loads and validates the checkout at `path` (default: the current directory), offline. |
| `aegis status` | Shows the commit the controller wants, what is wrong with it, and what each host reported. |
| `aegis diff [path]` | Compares the checkout at `path` with what the hosts last reported, and shows the override Aegis renders for each exposed stack. |
| `aegis sync [--host <name>] [--stack <name>]` | Fetches the repository and makes the agents converge now, instead of at the next poll. |

## Exit codes

Every command ends with one of three codes, so a script or a pre-push hook can tell a finding from a failure:

| Code | Meaning |
|------|---------|
| `0` | The command ran and found nothing. |
| `1` | The command ran and a check found something. |
| `2` | The command could not complete: a bad flag or path, a controller that cannot be reached or refuses the caller. |

What counts as a finding:

| Command | `1` when |
|---------|----------|
| `validate` | any stack is rejected. `2` when `path` is not a directory with a `hosts/` or `aegis/` directory. |
| `status` | no commit has loaded, the newest commit cannot be read, the remote cannot be fetched, a host has no agent address, has not confirmed the target commit or has a failing agent, or a stack is rejected, `failed` or `invalid`. |
| `diff` | a stack of the checkout is not `running` on its host (or a tombstoned one is not `purged`), a host the checkout declares stacks for has not reported, a host is at another commit than the controller's target, the controller has no target commit yet, an override cannot be rendered, a stack is rejected in the checkout, or a stack still `running`, `failed` or `invalid` on a host is not in the checkout. |
| `sync` | a host could not be synced, a stack failed or is invalid after the sync, or the sync reached no host. |

An `orphaned` or `purged` stack is the expected outcome of removing a stack ([agent.md](agent.md)), so it is listed by `status`, `diff` and `sync` and never makes them exit `1`.

## Reaching the controller

`status`, `diff` and `sync` call the controller at `--controller <url>`, or at `AEGIS_CONTROLLER` when the flag is absent, such as `http://100.64.0.2:7480`. There is no configuration file.

The controller recognises you by your Tailscale identity, not by a tag: a personal device that is tagged becomes a shared device, so an operator's laptop stays untagged. The controller lists the logins it accepts in `AEGIS_OPERATORS` ([controller.md](controller.md)), and only an untagged node of one of those people is an operator. An operator can read `GET /v1/status` and call the sync routes; it cannot report as an agent or enrol a sidecar. The ACL must also let your device reach the controller ([tailscale-acl.md](tailscale-acl.md)). A controller without `AEGIS_OPERATORS` admits no operator, and the command exits `2` with `403`.

## `aegis validate [path]`

Runs the loader, the validation rules and the compose allowlist, as an agent does at sync time, without a network or a controller. Each rejected stack is listed with its violations:

```text
invalid hosts/jao-server/broken
  hosts/jao-server/broken/compose.yaml: ...
1 rejected, 7 valid
```

A clean checkout prints `ok: <n> stacks`.

## `aegis status`

```text
target 3fe0623
jao-server  agent mapped  confirmed
  at 3fe0623
  hosts/jao-server/pihole  running
  hosts/jao-server/old  orphaned
    volume aegis-old_data
```

Rejected stacks of the target, an unreadable newest commit and a failed fetch are listed above the hosts. Each host line says whether the controller has an agent address for it and whether it confirmed the target commit (`confirmed`, `pending` or `failing: <reason>`).

## `aegis diff [path]`

Compares the stacks the checkout declares with the last report of each host, stack by stack:

```text
controller target 3fe0623
host jao-server
  at 3fe0623
  hosts/jao-server/pihole  same
  hosts/jao-server/web  not deployed
  hosts/jao-server/old  orphaned on the host, not in this checkout
override of hosts/jao-server/web (the tailnet name is a placeholder):
  services:
    tailscale:
      ...
```

The comparison is against the last report of each agent, so it is as fresh as the last reconcile; run `aegis sync` first for a current answer. The override is what Aegis renders for a stack with `expose` ([exposure.md](exposure.md)): the auth key is the `${AEGIS_TS_AUTHKEY}` placeholder and the tailnet name is a placeholder, because only the controller knows the real ones.

## `aegis sync [--host <name>] [--stack <name>]`

Asks the controller to fetch the repository now and make the agents converge to the newest commit, then waits for them. Without `--host` it syncs every host that has an agent, at the same time. `--stack` limits the reconcile to that one stack of each host it reaches: the others are left alone, nothing is taken down, and the sync does not change whether the host counts as confirmed (its report covers one stack only, so the agent does not report it to the controller). A host that has not confirmed the target commit is still reconciled whole at the next poll.

```text
jao-server: at 3fe0623
  hosts/jao-server/pihole  running
```

It exits `2` when the controller has no commit to sync to (`409`) or cannot fetch the repository (`502`). A host that is already reconciling is reported as not synced rather than queued.
