#!/usr/bin/env bash
# Tests scripts/image-tags.sh.
set -euo pipefail

SCRIPT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/image-tags.sh"
readonly SCRIPT

failures=0
expect() {
    local want="$1" got status=0
    shift
    got="$("${SCRIPT}" "$@" 2>/dev/null)" || status=$?
    if [[ "${got}" == "${want}" ]]; then
        echo "ok   $*"
    else
        echo "FAIL $* (status ${status}): '${got}', want '${want}'"
        failures=$((failures + 1))
    fi
}

expect $'0.1.0\n0.1\nlatest' v0.1.0
expect $'12.3.4\n12.3\nlatest' v12.3.4
expect "0.1.0-rc.2" v0.1.0-rc.2
expect "" 0.1.0
expect "" v0.1
expect "" v0.1.0-beta.1
expect "" v0.1.0-rc.1x
expect "" v0.1.0 extra
expect "" 

[[ ${failures} -eq 0 ]]
