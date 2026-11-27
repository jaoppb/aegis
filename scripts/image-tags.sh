#!/usr/bin/env bash
# Prints the container image tags a release publishes, one per line.
#
# Usage: image-tags.sh <tag>
#
# vX.Y.Z -> X.Y.Z, X.Y and latest; vX.Y.Z-rc.N -> X.Y.Z-rc.N only, so a release
# candidate never moves a floating tag.
set -euo pipefail

[ $# -eq 1 ] || {
    echo "Usage: $0 <tag>" >&2
    exit 1
}
if [[ "$1" =~ ^v([0-9]+)\.([0-9]+)\.([0-9]+)-rc\.([0-9]+)$ ]]; then
    echo "${BASH_REMATCH[1]}.${BASH_REMATCH[2]}.${BASH_REMATCH[3]}-rc.${BASH_REMATCH[4]}"
elif [[ "$1" =~ ^v([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
    echo "${BASH_REMATCH[1]}.${BASH_REMATCH[2]}.${BASH_REMATCH[3]}"
    echo "${BASH_REMATCH[1]}.${BASH_REMATCH[2]}"
    echo "latest"
else
    echo "Error: '$1' is not vX.Y.Z or vX.Y.Z-rc.N." >&2
    exit 1
fi
