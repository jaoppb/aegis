# Changelog

All notable changes to this project are documented in this file. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project follows [Semantic Versioning](https://semver.org/).

Every pull request that changes consumer-visible behaviour, the release flow or the CI workflows adds an entry under `Unreleased` (one bullet, ending `(#issue, #PR)`), or carries the `skip-changelog` label. `just bump-version` moves `Unreleased` under the new version heading, and the Release workflow publishes that section as the GitHub release notes.

## [Unreleased]

### Added

- Release pipeline: `just bump-version` opens a version bump pull request that rotates this changelog, merging it tags the version (`vX.Y.Z`, or `vX.Y.Z-rc.N` for a prerelease), and the Release workflow publishes this section as the release notes, static musl `aegis`, `aegis-controller` and `aegis-agent` binaries for amd64 and arm64, and multi-arch `ghcr.io/jaoppb/aegis-controller` and `aegis-agent` images bundling the `docker`, `sops` and `tailscale` versions pinned by the flake. The flake exposes the same binaries and images as packages (#8, #PR)

[Unreleased]: https://github.com/jaoppb/aegis/commits/main
