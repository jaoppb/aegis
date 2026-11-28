# Secrets with SOPS

Secrets live in the repository as [SOPS](https://github.com/getsops/sops) files encrypted with [age](https://age-encryption.org). Every agent holds its own age key and decrypts only what is encrypted to it, so the controller never sees plaintext and a host cannot read another host's secrets.

## What the agent decrypts

For each stack it deploys, the agent decrypts:

- every `env_file` entry of every service,
- every top-level `secrets` entry that reads a `file:`,
- the `.env` next to `compose.yaml`, which Compose itself reads for interpolation.

Validation already rejects a stack whose such file is not SOPS-encrypted (a format check, no decryption). At deploy time the agent runs `sops decrypt` on each file, writes the plaintext to a private directory on **tmpfs**, and runs Compose with a generated override file that points `env_file` and `secrets` at those copies (the decrypted `.env` is passed with `--env-file`). Compose and Docker never see the encrypted files, and nothing is written to the repository or to persistent disk.

The plaintext is removed as soon as `docker compose up` returns, whether it succeeded or failed. The agent also removes anything a crashed run left behind when it starts. A stack without any such file creates no directory and never runs `sops`.

The file format is taken from the extension: `.json`, `.yaml`/`.yml` and `.ini` use that format, and every other name, such as `.env` or `db.env`, is a dotenv file.

## Failures

A file that is not encrypted to this host's key, a missing required file, or a missing `sops` fails **only that stack**: its status is `failed` and the others still deploy. The message names the file and the exit status, never the output of `sops` (which can carry key material) and never a decrypted value. If Compose fails, every decrypted value is also removed from its message.

The decrypted `.env` may not set `COMPOSE_*` or `DOCKER_*` variables, nor `PATH`, `HOME`, `XDG_RUNTIME_DIR`, `SSL_CERT_FILE`, which Compose or Docker read: the stack is refused with the variable name (never its value), so a secret cannot redirect Docker. Variables in an `env_file` only reach the container, so they are not restricted.

## The agent's key

Generating the key and installing it is part of the bootstrap flow (#22). Until then, create it by hand on each host:

```sh
age-keygen -o /var/lib/aegis/age/keys.txt
```

and give the agent the path in `SOPS_AGE_KEY_FILE` (or `SOPS_AGE_KEY_CMD`, or `SOPS_AGE_SSH_PRIVATE_KEY_FILE`; without them `sops` looks in `$XDG_CONFIG_HOME/sops/age/keys.txt`). `sops` runs with a cleared environment: it inherits only `PATH`, `HOME`, `XDG_CONFIG_HOME`, `XDG_RUNTIME_DIR` and those three variables. `SOPS_AGE_KEY` is deliberately not passed, because a key held in a variable is easy to leak.

## tmpfs

The agent keeps plaintext under `$XDG_RUNTIME_DIR/aegis`, or `/dev/shm/aegis` when `XDG_RUNTIME_DIR` is unset. Before writing, it checks that the directory is on `tmpfs` or `ramfs` and refuses otherwise, failing the stack instead of writing plaintext to disk. Directories are `0700` and files `0600`.

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
