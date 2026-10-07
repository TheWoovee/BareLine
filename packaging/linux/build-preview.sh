#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# Build the unsigned Linux x64 preview on an x86_64 Linux host: a release build
# of the editor and the extension host, packed with their documents into
#   OUT/bareline-VERSION-linux-x64.tar.gz
# whose one top-level folder, bareline-VERSION-linux-x64/, holds
#   bareline, bareline-extension-host   as Cargo built them (not stripped)
#   bareline.portable                    empty marker: the profile stays in ./data
#   LICENSE, THIRD-PARTY-NOTICES.md, SBOM.json
#   bareline.desktop, bareline.png       desktop entry and icon
# Names are sorted, times are zero, the owner is root and gzip stores no name
# or time, so packing the same build again gives the same bytes. The archive is
# then listed, extracted and its editor asked for --version.
#
# Usage: packaging/linux/build-preview.sh [--out DIR] [--documents DIR] [--preview-tag TAG] [--commit SHA]
#   --out DIR          new output folder (default: dist/linux-preview)
#   --documents DIR    take THIRD-PARTY-NOTICES.md and SBOM.json from DIR instead
#                      of generating them, which needs pwsh and cargo-cyclonedx
#                      0.5.9 (packaging/windows/generate-preview-documents.ps1)
#   --preview-tag TAG  the v<version>-preview.<n> tag; GitHub Actions supplies a pushed tag
#   --commit SHA       the source commit, for a copy without .git (default: HEAD)
#
# Needs the Rust toolchain of rust-toolchain.toml, GNU tar, gzip and the build
# packages of the README ("Building on Linux and macOS"). Updates and extension
# loading are off in this build, as in the Windows preview.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
target="x86_64-unknown-linux-gnu"

out="dist/linux-preview"
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
            sed -n '4,25p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
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
need tar "it packs the archive"
need gzip "it compresses the archive"
need sha256sum "it reports the archive's hash"
tar --version 2> /dev/null | head -n 1 | grep -q 'GNU tar' || fail "GNU tar is required (--sort=name and the ustar options)"

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
folder="bareline-$version-linux-x64"

host="$(cd "$root" && rustc -vV | sed -n 's/^host: //p')"
[ "$host" = "$target" ] || fail "the Linux preview is built on $target; this host is '$host'"
cargo_target="${CARGO_TARGET_DIR:-$root/target}"
lock_hash="$(sha256sum "$root/Cargo.lock" | cut -d ' ' -f 1)"
mkdir -p "$root/target"
stage="$(mktemp -d "$root/target/linux-preview-packaging-XXXXXXXX")"

echo "Building unsigned Linux preview $preview_build_version from $preview_commit (updates and extension loading disabled)."
(
    cd "$root"
    unset BARELINE_PREPARED_RELEASE_CONFIG
    # About, --version and diagnostics identify the exact commit and preview.
    export BARELINE_BUILD_HASH="$preview_commit"
    export BARELINE_BUILD_VERSION="$preview_build_version"
    cargo build --locked --release -p bareline -p bareline-extension-host
)

if [ -z "$documents" ]; then
    need pwsh "it runs the notices and SBOM generators; or pass --documents"
    documents="$stage/documents"
    pwsh -NoProfile -NonInteractive -File "$root/packaging/windows/generate-preview-documents.ps1" -Target "$target" -OutputDir "$documents"
fi
for name in THIRD-PARTY-NOTICES.md SBOM.json; do
    [ -s "$documents/$name" ] || fail "$documents/$name is missing or empty"
done

payload="$stage/payload/$folder"
mkdir -p "$payload"
for name in bareline bareline-extension-host; do
    [ -s "$cargo_target/release/$name" ] || fail "$cargo_target/release/$name is missing"
    cp "$cargo_target/release/$name" "$payload/$name"
done
: > "$payload/bareline.portable"
cp "$root/LICENSE" "$documents/THIRD-PARTY-NOTICES.md" "$documents/SBOM.json" "$here/bareline.desktop" "$here/bareline.png" "$payload/"
chmod 0755 "$payload" "$payload/bareline" "$payload/bareline-extension-host"
chmod 0644 "$payload/bareline.portable" "$payload/LICENSE" "$payload/THIRD-PARTY-NOTICES.md" "$payload/SBOM.json" "$payload/bareline.desktop" "$payload/bareline.png"

mkdir -p "$out"
archive="$out/$folder.tar.gz"
LC_ALL=C tar --format=ustar --sort=name --mtime='@0' --owner=0 --group=0 --numeric-owner \
    -C "$stage/payload" -cf - "$folder" | gzip -9 -n > "$archive"

# The archive holds exactly the payload, and the extracted editor runs.
expected="$(cd "$stage/payload" && find "$folder" -print | sed "s|^$folder\$|$folder/|" | LC_ALL=C sort)"
listed="$(tar -tzf "$archive" | LC_ALL=C sort)"
[ "$expected" = "$listed" ] || fail "the archive does not list exactly the payload"
check="$stage/extracted"
mkdir -p "$check"
tar -xzf "$archive" -C "$check"
reported="$("$check/$folder/bareline" --version)"
[ "$reported" = "Bareline $preview_build_version ($preview_commit)" ] || fail "the extracted editor reports '$reported'"
[ ! -e "$check/$folder/data" ] || fail "--version created a profile folder"

[ "$(sha256sum "$root/Cargo.lock" | cut -d ' ' -f 1)" = "$lock_hash" ] || fail "Cargo.lock changed during packaging; review it before retrying"
echo "Unsigned Linux preview: $archive"
echo "SHA-256: $(sha256sum "$archive" | cut -d ' ' -f 1)"
echo "Staged payload, documents and the extracted check: $stage"
