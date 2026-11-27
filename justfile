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

# Run every check: format, lint and tests.
check: fmt-check lint test

# Audit the live GitHub repository settings, labels and milestones against tools.toml.
repo-check:
    tools repo check

# Audit open issues against the workflow rules (needs `gh` auth).
issues-check:
    tools issues check

# Apply tools.toml to the live repository. Changes GitHub settings and deletes deprecated labels.
repo-sync:
    tools repo sync
