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

# Lint the GitHub workflows and fail on unpinned or mismatched action references.
workflows-check:
    actionlint
    tools actions check

# Run every check: format, lint, workflow lint, architecture rules and tests.
check: fmt-check lint workflows-check arch-check test

# Audit the live GitHub repository settings, labels and milestones against tools.toml.
repo-check:
    tools repo check

# Fail when the change since `base` touches crates/, action.yml, .github/actions/ or .github/workflows/ without CHANGELOG.md (waived by the skip-changelog label in `labels`, comma-separated). Skipped while CHANGELOG.md does not exist (#8 creates it).
changelog-check base labels="":
    #!/usr/bin/env bash
    export BASE={{ quote(base) }} LABELS={{ quote(labels) }}
    set -euo pipefail
    if [ ! -f CHANGELOG.md ]; then
        echo "::notice::CHANGELOG.md does not exist yet; skipping the changelog check"
        exit 0
    fi
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
