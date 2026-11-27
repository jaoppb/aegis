#!/usr/bin/env bash
# Archives the static workspace binaries built by the flake for one target.
#
# Usage: package-binaries.sh <tag> <target-triple> <out-dir>
#
# Expects `nix build .#aegis -o result-aegis` (and the same for aegis-controller and
# aegis-agent) to have run in the current directory. Writes
# <out-dir>/aegis-<tag>-<target>.tar.gz and its .sha256, and checks that each binary is
# statically linked.
set -euo pipefail

die() {
    echo "Error: $*" >&2
    exit 1
}

[ $# -eq 3 ] || die "Usage: $0 <tag> <target-triple> <out-dir>"
TAG="$1"
TARGET="$2"
OUT="$3"
[[ "$TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$ ]] || die "'$TAG' is not vX.Y.Z or vX.Y.Z-rc.N."
[[ "$TARGET" =~ ^[a-z0-9_]+-unknown-linux-musl$ ]] || die "'$TARGET' is not a Linux musl target triple."

NAME="aegis-${TAG}-${TARGET}"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
mkdir -p "$STAGE/$NAME" "$OUT"

for bin in aegis aegis-controller aegis-agent; do
    src="result-${bin}/bin/${bin}"
    [ -x "$src" ] || die "$src not found: build it with 'nix build .#${bin} -o result-${bin}'."
    # A static binary has no dynamic section: `file` reports "statically linked" or "static-pie".
    file -L "$src" | grep -qE 'statically linked|static-pie linked' || die "$src is not statically linked."
    cp -L "$src" "$STAGE/$NAME/$bin"
done
cp LICENSE README.md "$STAGE/$NAME/"

tar -C "$STAGE" -czf "$OUT/$NAME.tar.gz" "$NAME"
(cd "$OUT" && sha256sum "$NAME.tar.gz" >"$NAME.tar.gz.sha256")
echo "Wrote $OUT/$NAME.tar.gz"
