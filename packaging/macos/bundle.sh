#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# Assemble Bareline.app from a release build, sign it ad hoc and wrap it in a
# disk image. Run from anywhere on macOS after one or both of:
#   cargo build --release --locked -p bareline -p bareline-extension-host --target aarch64-apple-darwin
#   cargo build --release --locked -p bareline -p bareline-extension-host --target x86_64-apple-darwin
# With both built, the app holds universal binaries (lipo); with one, those
# binaries; with neither, target/release (a host build) is used. The extension
# host goes beside the editor in Contents/MacOS when every selected build has it.
#
# Usage: packaging/macos/bundle.sh [--target TRIPLE]... [--out DIR] [--documents DIR] [--no-sign] [--no-dmg]
#   --target TRIPLE  use target/TRIPLE/release (repeat for universal)
#   --out DIR        output folder (default: target/macos)
#   --documents DIR  copy LICENSE, THIRD-PARTY-NOTICES.md and SBOM.json from DIR
#                    into Contents/Resources and beside the app in the disk image
#   --no-sign        skip ad-hoc signing (for checking the layout off macOS)
#   --no-dmg         skip the disk image
#
# Output: DIR/Bareline.app and DIR/bareline-VERSION-macos-ARCH.dmg, ARCH being
# arm64, x86_64 or universal. Every run starts from a clean bundle, so running
# it again gives the same result. Ad-hoc signing is for internal previews:
# Gatekeeper still blocks a downloaded copy until the release is signed with
# a Developer ID and notarized (CROSS_PLATFORM_PLAN ADR-D).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
bundle_identifier="com.thewoovee.bareline"
executable="bareline"
helper="bareline-extension-host"
documents_list="LICENSE THIRD-PARTY-NOTICES.md SBOM.json"

targets=()
out="$root/target/macos"
documents=""
sign=1
dmg=1
while [ "$#" -gt 0 ]; do
    case "$1" in
        --target)
            [ "$#" -ge 2 ] || { echo "bundle.sh: --target needs a triple" >&2; exit 2; }
            targets+=("$2")
            shift 2
            ;;
        --out)
            [ "$#" -ge 2 ] || { echo "bundle.sh: --out needs a folder" >&2; exit 2; }
            out="$2"
            shift 2
            ;;
        --documents)
            [ "$#" -ge 2 ] || { echo "bundle.sh: --documents needs a folder" >&2; exit 2; }
            documents="$2"
            shift 2
            ;;
        --no-sign) sign=0; shift ;;
        --no-dmg) dmg=0; shift ;;
        -h | --help)
            sed -n '4,24p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "bundle.sh: unknown argument: $1" >&2
            exit 2
            ;;
    esac
done

need() {
    command -v "$1" > /dev/null 2>&1 || { echo "bundle.sh: $1 is required ($2)" >&2; exit 1; }
}

if [ -n "$documents" ]; then
    for name in $documents_list; do
        [ -s "$documents/$name" ] || { echo "bundle.sh: $documents/$name is missing or empty" >&2; exit 1; }
    done
fi

# The version every crate shares: [workspace.package] version in Cargo.toml.
version="$(awk '
    { sub(/\r$/, "") }
    /^\[/ { section = $0 }
    section == "[workspace.package]" && $1 == "version" {
        gsub(/[" ]/, "", $3); print $3; exit
    }' "$root/Cargo.toml")"
if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "bundle.sh: no x.y.z version in [workspace.package] of Cargo.toml (found '$version')" >&2
    exit 1
fi

# The release folders to bundle from.
folders=()
# Array expansions are guarded by their length: macOS's bash 3.2 treats an
# empty array as unset under `set -u`.
if [ "${#targets[@]}" -eq 0 ]; then
    for triple in aarch64-apple-darwin x86_64-apple-darwin; do
        if [ -f "$root/target/$triple/release/$executable" ]; then
            targets+=("$triple")
        fi
    done
fi
if [ "${#targets[@]}" -gt 0 ]; then
    for triple in "${targets[@]}"; do
        binary="$root/target/$triple/release/$executable"
        [ -f "$binary" ] || { echo "bundle.sh: $binary is missing; build it first" >&2; exit 1; }
        folders+=("$root/target/$triple/release")
    done
fi
if [ "${#folders[@]}" -eq 0 ]; then
    binary="$root/target/release/$executable"
    [ -f "$binary" ] || { echo "bundle.sh: no release build of $executable under $root/target" >&2; exit 1; }
    folders+=("$root/target/release")
fi
# The extension host is bundled when every selected build has it.
programs="$executable"
helpers=0
for folder in "${folders[@]}"; do
    if [ -f "$folder/$helper" ]; then
        helpers=$((helpers + 1))
    fi
done
if [ "$helpers" -eq "${#folders[@]}" ]; then
    programs="$executable $helper"
elif [ "$helpers" -gt 0 ]; then
    echo "bundle.sh: $helper is built for some selected targets but not all" >&2
    exit 1
fi

work="$out/.work"
app="$out/Bareline.app"
rm -rf "$work" "$app"
mkdir -p "$work" "$app/Contents/MacOS" "$app/Contents/Resources"
trap 'rm -rf "$work"' EXIT

# 1. The executables: as built, or universal binaries from two builds.
case "${targets[0]:-host}" in
    aarch64-*) arch="arm64" ;;
    x86_64-*) arch="x86_64" ;;
    *) arch="$(uname -m)" ;;
esac
if [ "${#folders[@]}" -gt 1 ]; then
    need lipo "it merges the per-architecture builds"
    arch="universal"
fi
for program in $programs; do
    if [ "${#folders[@]}" -gt 1 ]; then
        inputs=()
        for folder in "${folders[@]}"; do
            inputs+=("$folder/$program")
        done
        lipo -create -output "$app/Contents/MacOS/$program" "${inputs[@]}"
    else
        cp "${folders[0]}/$program" "$app/Contents/MacOS/$program"
    fi
    chmod 755 "$app/Contents/MacOS/$program"
    if command -v lipo > /dev/null 2>&1; then
        lipo -info "$app/Contents/MacOS/$program"
    fi
done

# 2. The icon. Bareline has no macOS icon yet; until Bareline.icns is designed,
# the Windows artwork is converted when the system tools can, and otherwise
# the app keeps the generic application icon.
with_icon=0
icon="$here/Bareline.icns"
if [ ! -f "$icon" ] && command -v sips > /dev/null 2>&1 && command -v iconutil > /dev/null 2>&1; then
    iconset="$work/Bareline.iconset"
    mkdir -p "$iconset"
    if sips -s format png "$root/packaging/windows/bareline.ico" --out "$work/source.png" > /dev/null 2>&1; then
        for size in 16 32 128 256 512; do
            sips -z "$size" "$size" "$work/source.png" --out "$iconset/icon_${size}x${size}.png" > /dev/null
            double=$((size * 2))
            sips -z "$double" "$double" "$work/source.png" --out "$iconset/icon_${size}x${size}@2x.png" > /dev/null
        done
        iconutil -c icns "$iconset" -o "$work/Bareline.icns" && icon="$work/Bareline.icns"
    fi
fi
if [ -f "$icon" ]; then
    cp "$icon" "$app/Contents/Resources/Bareline.icns"
    with_icon=1
else
    echo "bundle.sh: no icon; the app uses the generic application icon (placeholder)"
fi

# 3. Info.plist and PkgInfo.
template="$here/Info.plist.in"
awk -v version="$version" -v identifier="$bundle_identifier" -v executable="$executable" -v icon="$with_icon" '
    /^@ICON_ENTRY@/ {
        if (icon == 1) printf "\t<key>CFBundleIconFile</key>\n\t<string>Bareline</string>\n"
        sub(/^@ICON_ENTRY@/, "")
    }
    {
        gsub(/@VERSION@/, version)
        gsub(/@BUNDLE_IDENTIFIER@/, identifier)
        gsub(/@EXECUTABLE@/, executable)
        print
    }' "$template" > "$app/Contents/Info.plist"
if grep -q '@[A-Z_]*@' "$app/Contents/Info.plist"; then
    echo "bundle.sh: unreplaced placeholder in Info.plist" >&2
    exit 1
fi
if command -v plutil > /dev/null 2>&1; then
    plutil -lint "$app/Contents/Info.plist"
fi
printf 'APPL????' > "$app/Contents/PkgInfo"

# 4. The license documents, sealed into the bundle by the signature below.
if [ -n "$documents" ]; then
    for name in $documents_list; do
        cp "$documents/$name" "$app/Contents/Resources/$name"
        chmod 644 "$app/Contents/Resources/$name"
    done
fi

# 5. Ad-hoc signature: the extension host as nested code, then the bundle.
if [ "$sign" -eq 1 ]; then
    need codesign "it signs the bundle; pass --no-sign to skip"
    for program in $programs; do
        if [ "$program" != "$executable" ]; then
            codesign --sign - --force --timestamp=none "$app/Contents/MacOS/$program"
        fi
    done
    codesign --sign - --force --deep --timestamp=none "$app"
    codesign --verify --deep --strict --verbose=2 "$app"
fi

# 6. The disk image: the app, a link to /Applications and the documents.
if [ "$dmg" -eq 1 ]; then
    need hdiutil "it builds the disk image; pass --no-dmg to skip"
    staging="$work/dmg"
    mkdir -p "$staging"
    cp -R "$app" "$staging/"
    ln -s /Applications "$staging/Applications"
    if [ -n "$documents" ]; then
        for name in $documents_list; do
            cp "$documents/$name" "$staging/$name"
        done
    fi
    image="$out/bareline-$version-macos-$arch.dmg"
    rm -f "$image"
    hdiutil create -volname "Bareline" -srcfolder "$staging" -ov -format UDZO "$image" > /dev/null
    hdiutil verify "$image" > /dev/null
    echo "bundle.sh: $image"
fi

echo "bundle.sh: $app ($version, $arch)"
