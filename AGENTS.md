# Aegis agent rules

Aegis is GitOps for Docker Compose stacks: a Rust workspace of a controller, per-host agents and a CLI. This file is the canonical rulebook for humans and agents. `CLAUDE.md` is a symlink to it, so every harness reads the same rules.

## 1. Issue workflow

- **Hierarchy.** Standalone issues under release milestones. Native GitHub sub-issues may break a task into steps.
- **Milestones.** Every open issue belongs to a release milestone declared in `tools.toml` `[github.milestones]`, the single source of truth. Docs and templates do not list milestones.
- **Labels.** `tools.toml` `[github.labels]` is the single source of truth, and `[github.issue_templates]` maps each template in `.github/ISSUE_TEMPLATE/` to its label and title prefix. `just repo-check` (`tools repo check`) fails when the templates or the list below drift from `tools.toml`. Every issue carries exactly one `type:` label and at least one `area:` label. `skip-changelog` is a pull request label, never an issue label.
<!-- labels:start -->
- `type:feat`
- `type:fix`
- `type:chore`
- `type:docs`
- `type:refactor`
- `type:spike`
- `area:domain`
- `area:agent`
- `area:controller`
- `area:cli`
- `area:secrets`
- `area:tailscale`
- `area:ui`
- `area:updates`
- `area:docs`
- `area:tooling`
- `accessibility`
- `skip-changelog`
<!-- labels:end -->
- **Creation and triage.** File issues from the matching template (it applies the `type:` label and title prefix), set the `area:` label(s), assign the release milestone, and link dependencies with native GitHub blocked-by links (also listed in the template's `Dependencies` field).
- **Project board.** `tools.toml` `[workflow]` defines the statuses and the Ready gate, and `[github.project]` points at the Aegis GitHub Project. The `Sync project` workflow (`.github/workflows/sync-project.yml`) moves cards on issue and pull request events and every six hours, using the `PROJECT_TOKEN` secret (a classic PAT with the `project` scope). `just project-check` shows the planned moves and `just project-sync` writes them; the latter needs a token with Projects write.

## 2. Branches, commits and pull requests

- Branch: `<type>/<issue>` (`feat/7`, `docs/3`).
- Commit and pull request title: `type: subject (#issue)`, where `type` is one of the `type:` labels.
- Close with `Closes #issue` in the pull request body and the branch's final commit; use `Refs #issue` on intermediate commits.
- Fill in `.github/pull_request_template.md`.
- Merge method: **merge commit only**. Squash and rebase are disabled and merged branches are deleted (`tools.toml` `[github.repository.settings]`), so every commit reaches `main`. Rebase your own branch on `origin/main` before the pull request, never rewrite it afterwards.
- **Changelog.** `CHANGELOG.md` (Keep a Changelog, `Unreleased` on top) is created by #8 and is not present yet. Once it exists, a pull request with a consumer-visible change adds one `Unreleased` bullet ending `(#issue, #PR)`, or carries the `skip-changelog` label.

## 3. Developer setup and Git hooks

- **Nix devShell (mandatory).** The `flake.nix` devShell, pinned by `flake.lock` and `rust-toolchain.toml`, is the only supported toolchain. Enter it with `nix develop` (or `direnv allow` with the checked-in `.envrc`), or run one recipe with `nix develop -c just <recipe>`. The justfile's `_devshell` guard makes every recipe fail outside it; only `just --list` works anywhere.
- **Setup.** `just setup` installs the Lefthook hooks. Hooks call `just` recipes only.
- **`pre-commit`**: `just fmt-check` (Rust and Markdown files) and `just lint` (Rust, TOML and lock files).
- **`pre-push`**: `just check`, `just issues-check` and `just repo-check`. The last two need network access and `gh` authentication.
- **Offline push.** When offline or unauthenticated, skip only the network audits: `LEFTHOOK_EXCLUDE=issues-check,repo-check git push`. Keep `check`, and run the skipped audits once you are back online. `git push --no-verify` is a last resort. Local hook overrides go in `lefthook-local.yml` (git-ignored).
- **Recipes.** `just fmt` formats Rust and Markdown (rumdl); `just lint` runs Clippy with warnings as errors; `just test` runs `cargo nextest`; `just arch-check` runs the architecture rules; `just check` runs `fmt-check`, `lint`, `arch-check` and `test`.
- **Repository sync.** `just repo-sync` applies `tools.toml` to live GitHub settings, labels and milestones and deletes deprecated labels. Run it only from an up-to-date `main` checkout, after the person asks for it.
- **CI.** The check workflows arrive with #6. Workflows live in `.github/workflows/` and must call `just` recipes through the devShell; do not duplicate check logic in a workflow: add a recipe and call it.

## 4. Architecture

The workspace follows clean architecture. Dependencies point inward: `domain` <- `application` <- `adapters` <- binaries (`cli`, `controller`, `agent`). `arch-lint.toml` declares the layers and `just arch-check` (the `tests/architecture` crate, part of `just check`) enforces them:

- `domain` depends on no other layer and is pure: no IO, process, environment, async runtime, HTTP, CLI, logging or serialization wire formats.
- `application` depends only on `domain`. It declares ports (traits) and does no IO.
- `adapters` implement application ports and never depend on a binary.
- Binaries only wire things together, hold no logic, and never depend on each other.

New IO goes behind a port in `application` and an adapter in `adapters`. Changing a rule means editing `arch-lint.toml` in a pull request that explains why.

## 5. Code and test conventions

- **Lints.** The workspace lint policy in `Cargo.toml` (`[workspace.lints]`) and `clippy.toml` is strict: Clippy `all`, `pedantic`, `nursery` and `cargo` are denied, and `unsafe`, `unwrap`/`expect`/`panic`, indexing, `as` conversions, printing, `todo!` and `dbg!` are forbidden outside tests, as are missing docs and `unreachable_pub`. Functions stay small, flat and low in complexity (see the thresholds in `clippy.toml`). Every crate under `crates/` opts in with `[lints] workspace = true`.
- **No `#[allow]`.** `#[allow]` is forbidden (`allow_attributes` and `allow_attributes_without_reason`); do not suppress a lint, fix the code.
- **Tests.** Run with `just test` (`cargo nextest`). Unit tests sit beside the code, cross-crate and architecture tests under `tests/`. Tests may use `unwrap`, `expect`, `panic` and printing. A bug fix starts with a failing test.
- **Formatting.** `rustfmt.toml` and `.rumdl.toml` are authoritative; run `just fmt`.

## 6. Product truth versus rules

`PRODUCT.md` is intended to hold product truth: what Aegis does and why, and its scope. It is not in the repository yet. This file holds coding and workflow rules. When they disagree about behaviour or scope, `PRODUCT.md` wins; about process, code style or tooling, `AGENTS.md` wins. Neither overrides code or GitHub issues without a pull request that changes them.
