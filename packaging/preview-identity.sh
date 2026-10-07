# SPDX-License-Identifier: MPL-2.0
# Sourced by packaging/linux/build-preview.sh and packaging/macos/build-preview.sh.
#
# preview_identity ROOT TAG COMMIT sets
#   preview_version        x.y.z from [workspace.package] in ROOT/Cargo.toml
#   preview_commit         COMMIT, or the checkout's HEAD when COMMIT is empty
#   preview_build_version  what About and --version show
# the way packaging/windows/build-preview.ps1 derives them: a
# v<version>-preview.<n>[.<n>...] tag (TAG, or the pushed tag in GitHub Actions)
# gives <version>-preview.<n>..., and an untagged build <version>-dev+<12 hex
# digits of the commit>. Works with the bash 3.2 that macOS ships.

preview_identity() {
    local root="$1" tag="$2" commit="$3" pattern
    preview_version="$(awk '
        { sub(/\r$/, "") }
        /^\[/ { section = $0 }
        section == "[workspace.package]" && $1 == "version" {
            gsub(/[" ]/, "", $3); print $3; exit
        }' "$root/Cargo.toml")"
    if ! [[ "$preview_version" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; then
        echo "preview: no x.y.z version in [workspace.package] of Cargo.toml (found '$preview_version')" >&2
        return 1
    fi
    if [ -z "$commit" ]; then
        if ! commit="$(git -C "$root" rev-parse --verify HEAD 2> /dev/null)"; then
            echo "preview: packaging records the source commit; run it from a Git checkout or pass --commit" >&2
            return 1
        fi
    fi
    if ! [[ "$commit" =~ ^[0-9a-f]{40}$ ]]; then
        echo "preview: expected a 40-digit lowercase commit hash (found '$commit')" >&2
        return 1
    fi
    if [ -z "$tag" ] && [ "${GITHUB_REF_TYPE:-}" = "tag" ]; then
        tag="${GITHUB_REF_NAME:-}"
    fi
    if [ -n "$tag" ]; then
        pattern="^v${preview_version//./\\.}-preview\\.[1-9][0-9]*(\\.(0|[1-9][0-9]*))*\$"
        if ! [[ "$tag" =~ $pattern ]]; then
            echo "preview: expected v$preview_version-preview.<number>[.<number>...] matching the Cargo version; received '$tag'" >&2
            return 1
        fi
        preview_build_version="${tag#v}"
    else
        preview_build_version="$preview_version-dev+${commit:0:12}"
    fi
    preview_commit="$commit"
}
