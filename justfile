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

# Fail if a crate breaks the clean-architecture layering declared in arch-lint.toml.
arch-check:
    cargo test -p aegis-architecture-tests

# Lint the GitHub workflows and release scripts (with their tests) and fail on unpinned or mismatched action references.
workflows-check:
    actionlint
    tools actions check
    shellcheck scripts/*.sh scripts/tests/*.sh
    scripts/tests/release-tag.test.sh
    scripts/tests/rotate-changelog.test.sh
    scripts/tests/release-notes-links.test.sh
    scripts/tests/image-tags.test.sh

# Run every check: format, lint, workflow lint, architecture rules and tests.
check: fmt-check lint workflows-check arch-check test

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

# Fail when the live `main` ruleset differs from .github/rulesets/main.json (needs `gh` auth with admin read).
ruleset-check:
    #!/usr/bin/env bash
    set -euo pipefail
    repo=$(gh repo view --json nameWithOwner --jq .nameWithOwner)
    id=$(gh api "repos/$repo/rulesets" --jq '.[] | select(.name == "main") | .id')
    if [ -z "$id" ]; then echo "no ruleset named main on $repo"; exit 1; fi
    norm='{name, target, enforcement, conditions, bypass_actors, rules} | walk(if type == "array" then sort_by(tostring) else . end)'
    live=$(gh api "repos/$repo/rulesets/$id" | jq -S "$norm")
    want=$(jq -S "$norm" .github/rulesets/main.json)
    if [ "$live" != "$want" ]; then
        diff <(echo "$want") <(echo "$live") || true
        echo "live ruleset differs from .github/rulesets/main.json"
        exit 1
    fi
    echo "ruleset main matches .github/rulesets/main.json"

# Create or update the `main` ruleset from .github/rulesets/main.json. Changes how every pull request merges.
ruleset-apply:
    #!/usr/bin/env bash
    set -euo pipefail
    repo=$(gh repo view --json nameWithOwner --jq .nameWithOwner)
    id=$(gh api "repos/$repo/rulesets" --jq '.[] | select(.name == "main") | .id')
    if [ -n "$id" ]; then method=PUT; url="repos/$repo/rulesets/$id"; else method=POST; url="repos/$repo/rulesets"; fi
    gh api --method "$method" "$url" --input .github/rulesets/main.json --silent
    echo "ruleset main applied to $repo"

# Open a pull request that bumps the workspace version (major, minor, patch, X.Y.Z or X.Y.Z-rc.N): `just bump-version minor --issue 90 [--dry-run]`.
bump-version target *args:
    ./scripts/bump-version.sh {{ target }} {{ args }}
