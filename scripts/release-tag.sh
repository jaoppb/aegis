#!/usr/bin/env bash
# Decides, creates and pushes the release tag for .github/workflows/tag-release.yml.
#
#   release-tag.sh push <before> <after>
#       A push to main touched Cargo.toml. Tag <after> only when the push changed
#       [workspace.package].version, so a later unrelated Cargo.toml change never
#       releases under a version bumped by an earlier commit.
#
#   release-tag.sh dispatch <tag> <ref> <default-ref>
#       Manual recovery. A missing <tag> is created at <ref> (or <default-ref> when
#       <ref> is empty) after checking the version there; an existing <tag> is
#       released as it is, and a non-empty <ref> must resolve to its commit.
#       A tag is never moved or recreated.
#
#   release-tag.sh verify <tag>
#       Fails unless <tag> is a release tag matching the [workspace.package].version of
#       the checked-out Cargo.toml and, unless it is a -rc.N tag, the checked-out commit is
#       on origin/main (release.yml runs it on the tag's checkout, with full history).
#
# When release.yml should run, appends `tag=<vX.Y.Z[-rc.N]>` to $GITHUB_OUTPUT (stdout when
# unset). Messages use GitHub workflow commands. Needs git (with an `origin` remote and
# the compared commits fetched) and python3 >= 3.11.
set -euo pipefail

readonly ZERO_SHA=0000000000000000000000000000000000000000
readonly RECOVERY="run the Tag Release workflow manually (workflow_dispatch) with that tag and the commit to release as ref"

notice() { echo "::notice::$*"; }
warning() { echo "::warning::$*"; }
die() {
  echo "::error::$*" >&2
  exit 1
}

usage() {
  echo "usage: $0 push <before> <after> | dispatch <tag> <ref> <default-ref> | verify <tag>" >&2
  exit 2
}

# Prints [workspace.package].version of Cargo.toml at <rev>; nothing when the file or
# the key is missing or the file is not valid TOML.
workspace_version() {
  local manifest
  manifest="$(git show "$1:Cargo.toml" 2>/dev/null)" || return 0
  python3 -c '
import sys, tomllib
try:
    version = tomllib.loads(sys.stdin.read())["workspace"]["package"]["version"]
except (tomllib.TOMLDecodeError, KeyError, TypeError):
    sys.exit(0)
if isinstance(version, str):
    print(version)
' <<<"${manifest}"
}

is_rc_tag() { [[ $1 =~ -rc\.[0-9]+$ ]]; }

# A stable release is cut only from a commit reachable from origin/main, so it went through the
# pull request rules. A release candidate may come from any commit: it is a prerelease with no
# floating image tag, which is how a release is rehearsed from a branch.
require_on_main() {
  local tag="$1" commit="$2"
  is_rc_tag "${tag}" && return 0
  git fetch --quiet origin main || die "Could not fetch origin/main to check that ${tag} is released from it"
  git merge-base --is-ancestor "${commit}" refs/remotes/origin/main 2>/dev/null ||
    git merge-base --is-ancestor "${commit}" FETCH_HEAD ||
    die "Commit ${commit} is not on origin/main; a stable release (${tag}) is cut only from main"
}

is_release_tag() { [[ $1 =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$ ]]; }

# Prints the commit <tag> points at on origin (peeled), nothing when it does not exist.
remote_tag_commit() {
  local refs
  refs="$(git ls-remote --tags origin "refs/tags/$1" "refs/tags/$1^{}")" ||
    die "Could not query tags on origin"
  # An annotated tag lists its peeled commit as `<tag>^{}`; a lightweight one only itself.
  awk -v tag="refs/tags/$1" '
    $2 == tag "^{}" { peeled = $1 }
    $2 == tag { direct = $1 }
    END { print (peeled != "" ? peeled : direct) }
  ' <<<"${refs}"
}

resolve_commit() {
  git rev-parse --verify --quiet "$1^{commit}" ||
    git rev-parse --verify --quiet "refs/remotes/origin/$1^{commit}" ||
    die "Cannot resolve '$1' to a commit"
}

create_tag() {
  local tag="$1" commit="$2"
  git -c user.name="github-actions[bot]" \
    -c user.email="41898282+github-actions[bot]@users.noreply.github.com" \
    tag -a "${tag}" -m "${tag}" "${commit}"
  git push origin "refs/tags/${tag}"
  notice "Tagged ${commit} as ${tag}"
}

release() { echo "tag=$1" >>"${GITHUB_OUTPUT:-/dev/stdout}"; }

cmd_push() {
  local before="$1" after="$2" version previous tag existing
  version="$(workspace_version "${after}")"
  if [[ -z ${version} ]]; then
    notice "Cargo.toml at ${after} has no [workspace.package].version; not tagging"
    return 0
  fi
  tag="v${version}"

  if [[ ${before} == "${ZERO_SHA}" ]] || ! git cat-file -e "${before}^{commit}" 2>/dev/null; then
    warning "Cannot compare with the commit before this push ('${before}'); not tagging ${tag}. If this push bumped the version, ${RECOVERY}"
    return 0
  fi

  previous="$(workspace_version "${before}")"
  # An assignment, not a test, so a failed query on origin stops the script.
  existing="$(remote_tag_commit "${tag}")"
  if [[ ${previous} == "${version}" ]]; then
    if [[ -z ${existing} ]]; then
      notice "This push did not change the workspace version ${version}; not tagging, although tag ${tag} is missing. To release it, ${RECOVERY}"
    else
      notice "This push did not change the workspace version ${version}; nothing to release"
    fi
    return 0
  fi

  if ! is_release_tag "${tag}"; then
    notice "Version '${version}' is neither X.Y.Z nor X.Y.Z-rc.N; not tagging"
    return 0
  fi
  if [[ -n ${existing} ]]; then
    notice "Tag ${tag} already exists; nothing to release"
    return 0
  fi
  require_on_main "${tag}" "${after}"
  create_tag "${tag}" "${after}"
  release "${tag}"
}

cmd_dispatch() {
  local tag="$1" ref="$2" default_ref="$3" existing commit version
  is_release_tag "${tag}" || die "Tag '${tag}' does not match 'v<major>.<minor>.<patch>[-rc.<n>]' (e.g. v0.1.0 or v0.1.0-rc.1)"

  existing="$(remote_tag_commit "${tag}")"
  if [[ -n ${existing} ]]; then
    # release.yml checks an existing tag against its Cargo.toml version.
    if [[ -n ${ref} ]]; then
      commit="$(resolve_commit "${ref}")"
      [[ ${commit} == "${existing}" ]] ||
        die "Tag ${tag} points at ${existing}, not at '${ref}' (${commit}); a tag is never moved"
    fi
    require_on_main "${tag}" "${existing}"
    notice "Tag ${tag} already exists at ${existing}; releasing it"
    release "${tag}"
    return 0
  fi

  commit="$(resolve_commit "${ref:-${default_ref}}")"
  version="$(workspace_version "${commit}")"
  [[ "v${version}" == "${tag}" ]] ||
    die "Cargo.toml at ${commit} has workspace version '${version}', which does not match ${tag}"
  require_on_main "${tag}" "${commit}"
  create_tag "${tag}" "${commit}"
  release "${tag}"
}

cmd_verify() {
  local tag="$1" version
  is_release_tag "${tag}" || die "Tag '${tag}' does not match 'v<major>.<minor>.<patch>[-rc.<n>]' (e.g. v0.1.0 or v0.1.0-rc.1)"
  version="$(workspace_version HEAD)"
  [[ "v${version}" == "${tag}" ]] ||
    die "Tag '${tag}' does not match the Cargo.toml workspace version '${version}' (expected 'v${version}')"
  require_on_main "${tag}" "$(git rev-parse HEAD)"
  notice "Release tag ${tag} matches the Cargo.toml workspace version ${version}"
}

case "${1:-}" in
  verify)
    [[ $# -eq 2 ]] || usage
    cmd_verify "$2"
    ;;
  push)
    [[ $# -eq 3 ]] || usage
    cmd_push "$2" "$3"
    ;;
  dispatch)
    [[ $# -eq 4 ]] || usage
    cmd_dispatch "$2" "$3" "$4"
    ;;
  *) usage ;;
esac
