# Secrets with SOPS

Secrets live in the repository as [SOPS](https://github.com/getsops/sops) files encrypted with [age](https://age-encryption.org). Every agent holds its own age key and decrypts only what is encrypted to it, so the controller never sees plaintext and a host cannot read another host's secrets.

## What the agent decrypts

For each stack it deploys, the agent decrypts:

- every `env_file` entry of every service,
- every top-level `secrets` entry that reads a `file:`,
- the `.env` next to `compose.yaml`, which Compose itself reads for interpolation.

Validation already rejects a stack when any of these files is not SOPS-encrypted (a format check, no decryption). At deploy time the agent runs `sops decrypt` on each file, writes the plaintext to a private directory on **tmpfs**, and runs Compose with a generated override file that points `env_file` and `secrets` at those copies (the decrypted `.env` is passed with `--env-file`). Compose and Docker never see the encrypted files, and nothing is written to the repository or to persistent disk.

The plaintext is removed as soon as `docker compose up` returns, whether it succeeded or failed. The agent also removes anything a crashed run left behind when it starts. A stack without any such file creates no directory and never runs `sops`.

The file format is taken from the extension: `.json`, `.yaml`/`.yml` and `.ini` use that format, and every other name, such as `.env` or `db.env`, is a dotenv file. So **every secret must be dotenv, JSON, YAML or INI content encrypted in that format**: a file `sops` encrypted as `binary` (its default for an unknown name, such as `cert.pem`) passes the format check but fails when the agent decrypts it as dotenv. To ship a PEM or another raw value, put it in a YAML or JSON file and encrypt that.

Only **age** recipients are accepted, and the check is strict because the file's author controls it: a key group other than age (`kms`, `gcp_kms`, `azure_kv`, `hc_vault`, `pgp`) makes `sops` call out to addresses or programs the author chose. For a YAML or JSON file, the agent parses it and requires a single document whose top-level `sops` key holds only the metadata keys `sops` writes for age (`age`, `mac`, `version`, `lastmodified`, the `*_suffix`, `*_regex` and `mac_only_encrypted` options, `shamir_threshold`, and `key_groups` made of `age` groups). A foreign group is tolerated only when empty (`null` or `[]`, as older `sops` wrote it). Anything else is refused: an unknown metadata key, a differently cased or escaped name, several documents, duplicate keys, an unparseable file, or a top-level key other than `sops` that is a case variant of it or is not ASCII. For a dotenv or INI file, the file is refused when its text contains `<group>__` anywhere (the flattened `sops_<group>__...` form) or when any key before `=` is not ASCII, so rename a data key that looks like that. A refused file fails its stack with "only age recipients are accepted".

The files must live inside the stack's own directory: the agent resolves every link first and refuses a file that leaves the directory (a committed `sub -> ../other-stack`) or is not a regular file.

## Failures

A file that is not encrypted to this host's key, a missing required file, or a missing `sops` fails **only that stack**: its status is `failed` and the others still deploy. The message names the file and the reason (the exit status, a launch error or the refusal of a non-age key group), never the output of `sops` (which can carry key material) and never a decrypted value. If Compose fails, the whole decrypted file and the value of each `NAME=value` line are removed from its message; a single value inside a JSON, YAML or INI secret file is not, so do not rely on the message to hide those.

The decrypted `.env` may not set `COMPOSE_*` or `DOCKER_*` variables, nor `PATH`, `HOME`, `XDG_RUNTIME_DIR` or `SSL_CERT_FILE`, which Compose or Docker read: the stack is refused with the variable name (never its value), so a secret cannot redirect Docker. It is read the way Compose reads it (`export NAME=`, `NAME: value`, a BOM, a bare `NAME` line), and a line that does not start with a plain name (`[A-Za-z0-9_.-]+`) refuses the stack, and a continuation line of a multi-line value usually fails this check, so keep multi-line values in a secret file instead. Variables in an `env_file` only reach the container, so they are not restricted.

## The agent's key

Generating the key and installing it is part of the bootstrap flow ([#22](https://github.com/jaoppb/aegis/issues/22)). Until then, create it by hand on each host:

```sh
age-keygen -o /var/lib/aegis/age/keys.txt
```

and give the agent the path in `SOPS_AGE_KEY_FILE` (or `SOPS_AGE_KEY_CMD`, or `SOPS_AGE_SSH_PRIVATE_KEY_FILE`; without them `sops` looks in `$XDG_CONFIG_HOME/sops/age/keys.txt`). `sops` runs with a cleared environment: it inherits only `PATH`, `HOME`, `XDG_CONFIG_HOME`, `XDG_RUNTIME_DIR` and those three variables. `SOPS_AGE_KEY` is deliberately not passed, because a key held in a variable is easy to leak.

## tmpfs

The agent keeps plaintext under `$XDG_RUNTIME_DIR/aegis`, or `/dev/shm/aegis` when `XDG_RUNTIME_DIR` is unset. Before writing, it checks that the directory is on `tmpfs` or `ramfs` and refuses otherwise, failing the stack instead of writing plaintext to disk. The directory and every project directory must be real directories owned by the agent with mode `0700` (it creates them so); anything else, such as a link or a directory prepared by another user in the shared `/dev/shm`, is refused. Files are `0600` and created fresh. Prefer a private `XDG_RUNTIME_DIR` over the `/dev/shm` fallback.

Docker bind-mounts `secrets` files from the host path, so the Docker daemon must see the same path. When the agent runs in a container, bind-mount a host tmpfs directory at the same path inside it, for example `-v /run/user/1000:/run/user/1000` with `XDG_RUNTIME_DIR=/run/user/1000`.

## `.sops.yaml`

Put a `.sops.yaml` at the repository root with one `creation_rules` entry per host, matching that host's directory, so a file under `hosts/<host>/` is encrypted to that host's key and nobody else's. Add a **backup recipient** to every rule: losing a host's age key makes its secrets unrecoverable unless another key can decrypt them. Keep the backup key offline, such as on a hardware token or in a password manager.

```yaml
creation_rules:
  - path_regex: ^hosts/jao-server/.*
    key_groups:
      - age:
          - age1hostjaoserver...   # jao-server's agent key
          - age1backupoffline...   # backup recipient, in every rule
  - path_regex: ^hosts/other-host/.*
    key_groups:
      - age:
          - age1hostotherhost...   # other-host's agent key
          - age1backupoffline...
```

`path_regex` is matched against the file path relative to where you run `sops`, so run it from the repository root. Then encrypt in place:

```sh
sops encrypt --in-place hosts/jao-server/vaultwarden/.env
sops encrypt --in-place --input-type dotenv --output-type dotenv hosts/jao-server/vaultwarden/db.secrets
```

The second form is for a name that is not `.env` or `*.env`, which `sops` would otherwise treat as binary. After adding or removing a recipient, re-encrypt the existing files with `sops updatekeys <file>`. To read a file yourself, use the backup key: `SOPS_AGE_KEY_FILE=backup.txt sops decrypt hosts/jao-server/vaultwarden/.env`.
