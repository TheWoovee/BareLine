# SPDX-License-Identifier: MPL-2.0
"""Validate the Scoop manifest template and fill it in for one verified preview.

The tag is read with the manifest's own `checkver.regex` and the download URL
comes from its `autoupdate` template, exactly as Scoop's autoupdate derives
them, so a template that could not follow a release fails here first. The
hash is taken from the verified artifact directory and must agree with its
SHA-256SUMS. Nothing is downloaded or published.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
TEMPLATE = ROOT / "packaging/scoop/bareline.json"
# Scoop evaluates .NET regexes; Python spells named groups (?P<name>...).
NET_NAMED_GROUP = re.compile(r"\(\?<(?![=!])")
SUMS_LINE = re.compile(r"([0-9a-f]{64})  ([A-Za-z0-9][A-Za-z0-9._-]*)")


class ManifestError(ValueError):
    pass


def require(condition, message):
    if not condition:
        raise ManifestError(message)


def read_manifest(path):
    manifest = json.loads(path.read_text(encoding="utf-8"))
    for key in ("version", "description", "homepage", "license", "architecture", "bin", "checkver", "autoupdate"):
        require(key in manifest, f"Scoop manifest lacks {key}")
    current = manifest["architecture"]["64bit"]
    require(re.fullmatch(r"[0-9a-f]{64}", current["hash"]), "Scoop manifest hash must be a lowercase SHA-256")
    require(set(manifest["checkver"]) == {"url", "jsonpath", "regex"}, "checkver must read GitHub releases (previews are prereleases)")
    # Scoop starts the app through its apps\bareline\current junction. The
    # portable marker would put profile and recovery data behind that reparse
    # point, so the installed-mode profile is used instead.
    require("bareline.portable" in manifest.get("post_install", ""), "post_install must remove the portable marker")
    # The checked-in release must be the one its own autoupdate rules produce.
    match = tag_match(manifest, "v" + manifest["version"])
    require(release_url(manifest, match) == current["url"], "Scoop manifest URL differs from its autoupdate template")
    return manifest


def tag_match(manifest, tag):
    match = re.search(NET_NAMED_GROUP.sub("(?P<", manifest["checkver"]["regex"]), tag)
    require(match is not None and match.groupdict().get("version") and match.groupdict().get("core"),
            f"checkver regex does not accept preview tag {tag!r}")
    return match


def substitute(template, match):
    values = {"$version": match["version"]}
    values.update({"$match" + name[:1].upper() + name[1:]: value for name, value in match.groupdict().items()})
    for key in sorted(values, key=len, reverse=True):
        template = template.replace(key, values[key])
    require("$" not in template, f"unsupported Scoop substitution in {template!r}")
    return template


def release_url(manifest, match):
    url = substitute(manifest["autoupdate"]["architecture"]["64bit"]["url"], match)
    require(url.startswith(manifest["homepage"] + "/releases/download/v" + match["version"] + "/"),
            "Scoop URL must be a GitHub release asset of the matched tag")
    return url


def render(template, artifact_dir, tag, cargo_version):
    manifest = read_manifest(template)
    match = tag_match(manifest, tag)
    require(match["core"] == cargo_version, f"tag {tag} does not carry Cargo version {cargo_version}")
    url = release_url(manifest, match)
    base, name = url.rsplit("/", 1)
    require(substitute(manifest["autoupdate"]["hash"]["url"].replace("$baseurl", base), match) == base + "/SHA-256SUMS",
            "Scoop hash must come from the release's SHA-256SUMS")
    archive = artifact_dir / name
    require(archive.is_file(), f"release does not contain the portable ZIP {name}")
    listed = {}
    for line in (artifact_dir / "SHA-256SUMS").read_text(encoding="utf-8").splitlines():
        entry = SUMS_LINE.fullmatch(line)
        require(entry is not None, "malformed SHA-256SUMS line")
        listed[entry[2]] = entry[1]
    with archive.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    require(listed.get(name) == digest, f"SHA-256SUMS does not list the actual hash of {name}")
    manifest["version"] = match["version"]
    manifest["architecture"] = {"64bit": {"url": url, "hash": digest}}
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--template", type=Path, default=TEMPLATE)
    parser.add_argument("--artifact-dir", type=Path, required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--cargo-version", required=True)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        manifest = render(arguments.template, arguments.artifact_dir, arguments.tag, arguments.cargo_version)
        require(not arguments.output.exists(), f"refusing to replace {arguments.output}")
        arguments.output.parent.mkdir(parents=True, exist_ok=True)
        arguments.output.write_text(json.dumps(manifest, indent=4) + "\n", encoding="utf-8")
    except (ManifestError, OSError, KeyError, json.JSONDecodeError) as error:
        print(f"Scoop manifest check failed: {error}", file=sys.stderr)
        return 1
    print(f"Scoop manifest {manifest['version']}: {manifest['architecture']['64bit']['url']} "
          f"sha256 {manifest['architecture']['64bit']['hash']} -> {arguments.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
