#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# Build the unsigned macOS preview for Apple silicon on a Mac: a release build
# of the editor and the extension host for aarch64-apple-darwin, bundled by
# bundle.sh into an ad-hoc signed Bareline.app inside
#   OUT/bareline-VERSION-macos-arm64.dmg
# with LICENSE, THIRD-PARTY-NOTICES.md and SBOM.json in Contents/Resources and
# beside the app. The disk image is then mounted read-only and checked: its
# entries, the arm64 executables, the signature, the documents, and that the
# editor reports this build's --version.
#
# Usage: packaging/macos/build-preview.sh [--out DIR] [--documents DIR] [--preview-tag TAG] [--commit SHA]
#   --out DIR          new output folder for the disk image (default: dist/macos-preview)
#   --documents DIR    take THIRD-PARTY-NOTICES.md and SBOM.json from DIR instead
#                      of generating them, which needs pwsh and cargo-cyclonedx
#                      0.5.9 (packaging/windows/generate-preview-documents.ps1)
#   --preview-tag TAG  the v<version>-preview.<n> tag; GitHub Actions supplies a pushed tag
#   --commit SHA       the source commit, for a copy without .git (default: HEAD)
#
# Needs the Rust toolchain of rust-toolchain.toml with the aarch64-apple-darwin
# target and the Xcode Command Line Tools. Updates and extension loading are
# off in this build, as in the Windows preview.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
target="aarch64-apple-darwin"

out="dist/macos-preview"
documents=""
tag=""
commit=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        --out | --documents | --preview-tag | --commit)
            [ "$#" -ge 2 ] || { echo "build-preview.sh: $1 needs a value" >&2; exit 2; }
            case "$1" in
                --out) out="$2" ;;
                --documents) documents="$2" ;;
                --preview-tag) tag="$2" ;;
                --commit) commit="$2" ;;
            esac
            shift 2
            ;;
        -h | --help)
            sed -n '4,23p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "build-preview.sh: unknown argument: $1" >&2
            exit 2
            ;;
    esac
done

fail() {
    echo "build-preview.sh: $1" >&2
    exit 1
}
need() {
    command -v "$1" > /dev/null 2>&1 || fail "$1 is required ($2)"
}
need cargo "it builds the editor"
need hdiutil "it mounts the disk image for the check"
need codesign "it checks the signature"
need lipo "it checks the architecture"
need shasum "it reports the disk image's hash"

absolute() {
    case "$1" in
        /*) printf '%s\n' "$1" ;;
        *) printf '%s/%s\n' "$(pwd)" "$1" ;;
    esac
}
out="$(absolute "$out")"
[ -z "$documents" ] || documents="$(absolute "$documents")"
[ ! -e "$out" ] || fail "use a new output folder: $out"

# shellcheck source=packaging/preview-identity.sh
. "$root/packaging/preview-identity.sh"
preview_identity "$root" "$tag" "$commit"
version="$preview_version"
image_name="bareline-$version-macos-arm64.dmg"

lock_hash="$(shasum -a 256 "$root/Cargo.lock" | cut -d ' ' -f 1)"
mkdir -p "$root/target"
stage="$(mktemp -d "$root/target/macos-preview-packaging-XXXXXXXX")"

echo "Building unsigned macOS preview $preview_build_version from $preview_commit (updates and extension loading disabled)."
(
    cd "$root"
    unset BARELINE_PREPARED_RELEASE_CONFIG CARGO_TARGET_DIR
    # About, --version and diagnostics identify the exact commit and preview.
    export BARELINE_BUILD_HASH="$preview_commit"
    export BARELINE_BUILD_VERSION="$preview_build_version"
    cargo build --locked --release --target "$target" -p bareline -p bareline-extension-host
)

if [ -z "$documents" ]; then
    need pwsh "it runs the notices and SBOM generators; or pass --documents"
    documents="$stage/documents"
    pwsh -NoProfile -NonInteractive -File "$root/packaging/windows/generate-preview-documents.ps1" -Target "$target" -OutputDir "$documents"
fi
for name in THIRD-PARTY-NOTICES.md SBOM.json; do
    [ -s "$documents/$name" ] || fail "$documents/$name is missing or empty"
done
# bundle.sh takes all three documents from one folder.
bundle_documents="$stage/bundle-documents"
mkdir -p "$bundle_documents"
cp "$root/LICENSE" "$documents/THIRD-PARTY-NOTICES.md" "$documents/SBOM.json" "$bundle_documents/"

"$here/bundle.sh" --target "$target" --out "$stage/bundle" --documents "$bundle_documents"
[ -s "$stage/bundle/$image_name" ] || fail "bundle.sh did not produce $image_name"

# Check the disk image as a user receives it.
mount_point="$stage/mounted"
mkdir -p "$mount_point"
hdiutil attach -readonly -nobrowse -noautoopen -mountpoint "$mount_point" "$stage/bundle/$image_name" > /dev/null
trap 'hdiutil detach "$mount_point" -quiet > /dev/null 2>&1 || hdiutil detach "$mount_point" -force > /dev/null 2>&1 || true' EXIT
listed="$(cd "$mount_point" && ls -1 | LC_ALL=C sort | tr '\n' ' ')"
[ "$listed" = "Applications Bareline.app LICENSE SBOM.json THIRD-PARTY-NOTICES.md " ] || fail "the disk image holds '$listed'"
[ -L "$mount_point/Applications" ] && [ "$(readlink "$mount_point/Applications")" = "/Applications" ] || fail "Applications is not a link to /Applications"
app="$mount_point/Bareline.app"
for program in bareline bareline-extension-host; do
    archs="$(lipo -archs "$app/Contents/MacOS/$program")"
    [ "$archs" = "arm64" ] || fail "$program is built for '$archs', not arm64"
done
for name in LICENSE THIRD-PARTY-NOTICES.md SBOM.json; do
    cmp -s "$bundle_documents/$name" "$mount_point/$name" || fail "$name beside the app differs from the packaged document"
    cmp -s "$bundle_documents/$name" "$app/Contents/Resources/$name" || fail "$name in Contents/Resources differs from the packaged document"
done
codesign --verify --deep --strict --verbose=2 "$app"
reported="$("$app/Contents/MacOS/bareline" --version)"
[ "$reported" = "Bareline $preview_build_version ($preview_commit)" ] || fail "the bundled editor reports '$reported'"
hdiutil detach "$mount_point" -quiet
trap - EXIT

[ "$(shasum -a 256 "$root/Cargo.lock" | cut -d ' ' -f 1)" = "$lock_hash" ] || fail "Cargo.lock changed during packaging; review it before retrying"
mkdir -p "$out"
cp "$stage/bundle/$image_name" "$out/$image_name"
echo "Unsigned macOS preview: $out/$image_name"
echo "SHA-256: $(shasum -a 256 "$out/$image_name" | cut -d ' ' -f 1)"
echo "Staged bundle and documents: $stage"
