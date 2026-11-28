set shell := ["bash", "-euo", "pipefail", "-c"]

# Recipes run only inside the flake devShell (`nix develop`, or direnv with `.envrc`), so local runs
# and CI use the same toolchain, pinned by flake.lock and rust-toolchain.toml. `eager` makes just
# evaluate this guard before any recipe, new ones included; `just --list` still works anywhere.
# flake.nix sets AEGIS_DEVSHELL.
eager _devshell := if env("AEGIS_DEVSHELL", "") == "" { error("run just inside the flake devShell: 'nix develop -c just <recipe>', or enter it with 'nix develop' or 'direnv allow'") } else { "" }

# Show the available recipes.
default:
    @just --list

# Install the git hooks.
setup:
    lefthook install

# Format Rust and Markdown in place.
fmt:
    cargo fmt --all
    rumdl fmt .

# Fail if Rust or Markdown is not formatted.
fmt-check:
    cargo fmt --all --check
    rumdl check .

# Lint the workspace with Clippy; warnings are errors.
lint:
    cargo clippy --workspace --all-targets --all-features -- -D warnings

# Run the test suite.
test:
    cargo nextest run --workspace --all-features --no-tests=pass

# Fail if a crate breaks the layering in arch-lint.toml or a structural rule of AGENTS.md section 5.
arch-check:
    cargo test -p aegis-architecture-tests

# Lint the GitHub workflows and release scripts (with their tests) and fail on unpinned or mismatched action references.
workflows-check:
    actionlint
    tools actions check
    shellcheck scripts/*.sh scripts/tests/*.sh
    for t in scripts/tests/*.test.sh; do "$t"; done

# Fail on a banned crate, a disallowed license, an unknown source or a duplicate crate version (deny.toml; no advisory database fetch).
deny:
    cargo deny check bans licenses sources

# Fail on a RustSec advisory against a locked dependency (needs network for the advisory database).
deny-advisories:
    cargo deny check advisories

# Run every check that needs no network: format, lint, workflow lint, dependency policy, architecture rules and tests.
check-offline: fmt-check lint workflows-check deny arch-check test

# Run every check: `check-offline` plus the advisory audit.
check: check-offline deny-advisories

# Audit the live GitHub repository settings, labels and milestones against tools.toml.
repo-check:
    tools repo check

# Fail when the change since `base` touches crates/, action.yml, .github/actions/ or .github/workflows/ without CHANGELOG.md (waived by the skip-changelog label in `labels`, comma-separated).
changelog-check base labels="":
    #!/usr/bin/env bash
    export BASE={{ quote(base) }} LABELS={{ quote(labels) }}
    set -euo pipefail
    tools changelog check --base "$BASE" --labels "$LABELS" --skip-label skip-changelog --changelog CHANGELOG.md --watch crates/ --watch action.yml --watch .github/actions/ --watch .github/workflows/

# Audit open issues against the workflow rules (needs `gh` auth).
issues-check:
    tools issues check

# Apply tools.toml to the live repository. Changes GitHub settings and deletes deprecated labels.
repo-sync:
    tools repo sync

# Print the planned Project Status moves without writing (exits 1 when there are any).
project-check:
    tools project check

# Write the planned Project Status moves (needs a token with Projects write).
project-sync:
    tools project sync

# Fail when a live ruleset differs from its file under .github/rulesets/ (needs `gh` auth with admin read).
ruleset-check:
    #!/usr/bin/env bash
    set -euo pipefail
    repo=$(gh repo view --json nameWithOwner --jq .nameWithOwner)
    norm='{name, target, enforcement, conditions, bypass_actors, rules} | walk(if type == "array" then sort_by(tostring) else . end)'
    list=$(gh api "repos/$repo/rulesets")
    status=0
    for file in .github/rulesets/*.json; do
        name=$(jq -r .name "$file")
        id=$(jq -r --arg name "$name" '.[] | select(.name == $name) | .id' <<<"$list")
        if [ -z "$id" ]; then echo "no ruleset named $name on $repo"; status=1; continue; fi
        live=$(gh api "repos/$repo/rulesets/$id" | jq -S "$norm")
        want=$(jq -S "$norm" "$file")
        if [ "$live" != "$want" ]; then
            diff <(echo "$want") <(echo "$live") || true
            echo "live ruleset $name differs from $file"
            status=1
        else
            echo "ruleset $name matches $file"
        fi
    done
    exit "$status"

# Create or update every ruleset under .github/rulesets/. Changes how pull requests merge and who may create release tags.
ruleset-apply:
    #!/usr/bin/env bash
    set -euo pipefail
    repo=$(gh repo view --json nameWithOwner --jq .nameWithOwner)
    for file in .github/rulesets/*.json; do
        if jq -e '.bypass_actors[]? | select(.actor_id == 0)' "$file" >/dev/null; then
            echo "$file has a bypass actor with actor_id 0: set the release GitHub App id first"
            exit 1
        fi
    done
    list=$(gh api "repos/$repo/rulesets")
    for file in .github/rulesets/*.json; do
        name=$(jq -r .name "$file")
        id=$(jq -r --arg name "$name" '.[] | select(.name == $name) | .id' <<<"$list")
        if [ -n "$id" ]; then method=PUT; url="repos/$repo/rulesets/$id"; else method=POST; url="repos/$repo/rulesets"; fi
        gh api --method "$method" "$url" --input "$file" --silent
        echo "ruleset $name applied to $repo"
    done

# Open a pull request that bumps the workspace version (major, minor, patch, X.Y.Z or X.Y.Z-rc.N): `just bump-version minor --issue 90 [--dry-run]`.
bump-version target *args:
    ./scripts/bump-version.sh {{ target }} {{ args }}
