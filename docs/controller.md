# Running the controller

The controller polls the GitOps repository, loads and validates each new commit, tells every agent to converge to it, serves the status of the fleet and enrols the Tailscale sidecars of exposed stacks (`POST /v1/enroll`, see [exposure.md](exposure.md)). One router serves `/v1/status` and `/v1/enroll`, both behind the tag check that admits only `tag:aegis-agent` peers. It keeps no state on disk: a restart looks at the repository again and tells the agents again, and a reconcile is idempotent.

## Configuration

| Variable | Required | Meaning |
|----------|----------|---------|
| `AEGIS_LISTEN` | yes | The socket address to serve on, normally the tailnet address, such as `100.64.0.7:7480`. |
| `AEGIS_REPO_URL` | yes | The repository to follow. A URL with credentials (`https://token@host/...`, or a password in any URL) is refused. |
| `AEGIS_WORKDIR` | yes | An absolute path for the disposable checkout. It is rebuilt when missing or damaged, and everything in it that the commit does not hold is deleted, so give it a directory of its own: a non-empty directory, or a git repository the controller did not create (it marks its own with `.git/aegis-managed`), is refused. |
| `AEGIS_REPO_BRANCH` | no | The branch to follow. Default `main`. |
| `AEGIS_POLL_SECS` | no | Seconds between two looks at the repository. Default `60`, at least `5`. |
| `AEGIS_TS_CLIENT_ID`, `AEGIS_TS_CLIENT_SECRET` | yes | The Tailscale OAuth client the controller mints sidecar keys with for `POST /v1/enroll` ([exposure.md](exposure.md)). The controller does not start without them. |
| `AEGIS_ENROLL_TAGS` | yes | Comma-separated ACL tags a stack's node may carry, such as `tag:app` ([exposure.md](exposure.md)). The controller does not start without it. |
| `AEGIS_AGENTS` | no | The agents, as comma-separated `host=url` pairs such as `jao-server=http://100.64.0.7:7480`. A URL with credentials is refused. A host of the repository without an entry is never told to reconcile. |

Git authenticates with what it inherits from the controller's environment: a deploy key through the SSH agent or `~/.ssh/config`, or a git credential helper. Aegis never reads, stores or logs a token. Only the `https`, `ssh` and `file` transports are allowed. Write access is not needed.

## What it does

Every poll interval the controller fetches the branch and loads the newest commit:

- A commit that loads becomes the target, even if some stacks in it are rejected: the rejected stacks are listed in the status, and the agents skip them and report them `invalid`.
- A commit whose tree cannot be read at all (the `hosts/` or `aegis/` directory is unreadable) is reported as `invalid` in the status, and the last good commit stays the target.
- A failed fetch is reported as `fetch_error` and changes nothing: the target and the running stacks stay as they are.

Every host with an agent that has not reported the target commit is told to converge with `POST /v1/reconcile`, and told again every poll until it reports. A call to one agent runs on its own, so a slow deploy delays neither the other hosts nor the next poll. While a call is still running (it may take up to ten minutes), the host is not told about a newer commit.

## `/v1/status`

Both methods are callable only by a node tagged `tag:aegis-agent` (see [tailscale-acl.md](tailscale-acl.md)). Every agent can therefore read the whole status, including the violations of every host's stacks, and report for any host; the tag is the trust boundary for now.

`POST /v1/status` takes the status report of an agent (see [agent.md](agent.md)); agents send it after each reconcile.

| Status | Meaning |
|--------|---------|
| `204` | The report was kept. It confirms the host only when it is for the target commit. |
| `400` | The host name or the commit is malformed. |
| `403` | The caller is not tagged `tag:aegis-agent`. |
| `404` | The host has no entry in `AEGIS_AGENTS`. |
| `503` | The whois lookup failed. |

`GET /v1/status` answers the commit the hosts should be at and what each host reported. Absent values are `null`:

```json
{
  "target": {"commit": "3fe0623", "rejected": [{"directory": "hosts/jao-server/broken", "state": "invalid", "details": ["..."]}]},
  "invalid": null,
  "fetch_error": null,
  "hosts": [
    {"host": "jao-server", "agent": "mapped", "confirmation": "confirmed", "report": {"host": "jao-server", "commit": "3fe0623", "stacks": []}}
  ]
}
```

- `target` is the last good commit with its rejected stacks. It stays in force while `invalid` names a newer commit that cannot be read, or `fetch_error` says why the remote cannot be reached.
- `agent` is `mapped` or `unmapped` (the repository declares the host, but `AEGIS_AGENTS` has no address for it).
- `confirmation` is `"confirmed"` (the host reported the target commit), `"pending"`, or `{"failing": "<reason>"}` (the last call to its agent failed).
- `report` is the last report of the host, for any commit.
