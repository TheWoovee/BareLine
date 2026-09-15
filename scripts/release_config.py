#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0
"""Validate and prepare Bareline's public release configuration and evidence."""

from __future__ import annotations

import argparse
import base64
import hashlib
import ipaddress
import json
import os
import re
import stat
import subprocess
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCHEMA = ROOT / "release" / "release-config.schema.json"
PREPARED_KEYS = (
    "BARELINE_BUILD_MODE", "BARELINE_CONFIG_VERSION", "BARELINE_CONFIG_DIGEST",
    "BARELINE_SOURCE_REVISION", "BARELINE_SOURCE_DIGEST", "BARELINE_RELEASE_VERSION",
    "BARELINE_RELEASE_CHANNEL", "BARELINE_RELEASE_PUBLIC_KEY", "BARELINE_CATALOG_PUBLIC_KEY",
    "BARELINE_PUBLISHER", "BARELINE_PUBLISHER_CERT_SHA256", "BARELINE_METADATA_FLOOR",
    "BARELINE_UPDATE_HOST", "BARELINE_UPDATE_MANIFEST_PATH", "BARELINE_UPDATE_SIGNATURE_PATH",
    "BARELINE_UPDATE_ARTIFACT_PATH", "BARELINE_BUILD_FEATURES",
    "BARELINE_CONFIG_JSON_B64", "BARELINE_SOURCE_IDENTITY_ALGORITHM",
    "BARELINE_SOURCE_WORKING_TREE_DIRTY",
)
SOURCE_IDENTITY_ALGORITHM = "git-status-diff-untracked-v1"

NONSHIPPING_VALUES = {
    "RWRURVNUT05MWQOhB7/zzhC+HXDdGOdLwJln5NYwm6UNXx3chmQSVTG4",
    "RWQHBwcHBwcHBxl/ayPhbIUyxqvIOPrNXqeJvgx2spIDNAOb+os9No1h",
    "0707070707070707070707070707070707070707070707070707070707070707",
}


class ConfigurationError(ValueError):
    pass


def load_json_bytes(raw: bytes, label: str) -> object:
    if len(raw) > 64 * 1024:
        raise ConfigurationError(f"{label}: JSON exceeds 64 KiB")
    try:
        def unique(pairs: list[tuple[str, object]]) -> dict:
            result = {}
            for key, value in pairs:
                if key in result:
                    raise ConfigurationError(f"{label}: duplicate JSON key {key!r}")
                result[key] = value
            return result
        return json.loads(raw.decode("utf-8"), object_pairs_hook=unique)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ConfigurationError(f"{label}: invalid UTF-8 JSON: {error}") from error


def load_json(path: Path) -> object:
    return load_json_bytes(path.read_bytes(), str(path))


def schema_validate(value: object, schema: dict, location: str = "$") -> None:
    expected = schema.get("type")
    if expected == "object" and not isinstance(value, dict):
        raise ConfigurationError(f"{location}: expected object")
    if expected == "array" and not isinstance(value, list):
        raise ConfigurationError(f"{location}: expected array")
    if expected == "string" and not isinstance(value, str):
        raise ConfigurationError(f"{location}: expected string")
    if expected == "integer" and (not isinstance(value, int) or isinstance(value, bool)):
        raise ConfigurationError(f"{location}: expected integer")
    if expected == "boolean" and not isinstance(value, bool):
        raise ConfigurationError(f"{location}: expected boolean")
    if "const" in schema and value != schema["const"]:
        raise ConfigurationError(f"{location}: expected {schema['const']!r}")
    if "enum" in schema and value not in schema["enum"]:
        raise ConfigurationError(f"{location}: unsupported value {value!r}")
    if isinstance(value, int) and "minimum" in schema and value < schema["minimum"]:
        raise ConfigurationError(f"{location}: below minimum {schema['minimum']}")
    if isinstance(value, str) and "pattern" in schema and re.fullmatch(schema["pattern"], value) is None:
        raise ConfigurationError(f"{location}: value does not match required format")
    if isinstance(value, dict):
        required = set(schema.get("required", []))
        missing = sorted(required - value.keys())
        if missing:
            raise ConfigurationError(f"{location}: missing {', '.join(missing)}")
        properties = schema.get("properties", {})
        if schema.get("additionalProperties") is False:
            unknown = sorted(value.keys() - properties.keys())
            if unknown:
                raise ConfigurationError(f"{location}: unknown {', '.join(unknown)}")
        for key, child in value.items():
            if key in properties:
                schema_validate(child, properties[key], f"{location}.{key}")
    if isinstance(value, list) and "items" in schema:
        for index, child in enumerate(value):
            schema_validate(child, schema["items"], f"{location}[{index}]")


def valid_minisign_key(value: str) -> bool:
    if len(value) != 56 or not value.startswith("RW"):
        return False
    try:
        decoded = base64.b64decode(value, validate=True)
    except ValueError:
        return False
    return len(decoded) == 42 and decoded[:2] == b"Ed"


def workspace_version() -> str:
    text = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    match = re.search(r"(?m)^version\s*=\s*\"([^\"]+)\"", text)
    if not match:
        raise ConfigurationError("workspace package version is missing")
    return match.group(1)


def validate_document(document: object, allowed_modes: set[str] | None = None) -> dict:
    schema = load_json(SCHEMA)
    assert isinstance(schema, dict)
    schema_validate(document, schema)
    assert isinstance(document, dict)
    mode = document["mode"]
    if allowed_modes is not None and mode not in allowed_modes:
        raise ConfigurationError(f"$.mode: expected one of {sorted(allowed_modes)}, got {mode!r}")
    distribution = document["distribution"]
    features = document["features"]
    trust = document["trust"]
    updates = document["updates"]
    if distribution["version"] != workspace_version():
        raise ConfigurationError("$.distribution.version: must equal the workspace package version")
    version = distribution["version"]
    channel = distribution["channel"]
    if (channel == "stable" and "-" in version) or (channel == "beta" and "-beta." not in version):
        raise ConfigurationError("$.distribution: channel and semantic version disagree")
    if mode == "preview":
        if channel != "preview" or features != {"updates": False, "extensions": False, "external_runtime": True}:
            raise ConfigurationError("preview builds must disable update/extension trust and declare an external runtime")
        if any(trust.values()) or any(updates[key] for key in ("host", "manifest_path", "signature_path", "artifact_path")):
            raise ConfigurationError("preview configuration must not carry trust pins or endpoints")
        return document
    if channel == "preview":
        raise ConfigurationError("configured and fixture builds cannot use the preview channel")
    if features != {"updates": True, "extensions": True, "external_runtime": True}:
        raise ConfigurationError("configured and fixture builds require updates, extensions, and a separate runtime")
    for key in ("release_public_key", "catalog_public_key"):
        if not valid_minisign_key(trust[key]):
            raise ConfigurationError(f"$.trust.{key}: expected a 56-character Minisign Ed25519 public key")
    if trust["release_public_key"] == trust["catalog_public_key"]:
        raise ConfigurationError("release and catalog keys must be independently rotatable")
    if mode == "configured" and any(value in NONSHIPPING_VALUES for value in trust.values()):
        raise ConfigurationError("configured releases cannot use checked-in nonshipping trust")
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9 ._-]{1,126}[A-Za-z0-9]", trust["publisher"]):
        raise ConfigurationError("$.trust.publisher: invalid publisher identity")
    if not re.fullmatch(r"[0-9a-f]{64}", trust["publisher_certificate_sha256"]):
        raise ConfigurationError("$.trust.publisher_certificate_sha256: expected lowercase SHA-256")
    host = updates["host"]
    reserved = (".invalid", ".example", ".test", ".localhost")
    labels = host.split(".")
    if mode == "configured" and (
        host != host.lower()
        or len(host) > 253
        or len(labels) < 2
        or any(not re.fullmatch(r"[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?", label) for label in labels)
        or host == "localhost"
        or host.endswith(reserved)
    ):
        raise ConfigurationError("$.updates.host: configured releases require a real public DNS host")
    try:
        ipaddress.ip_address(host)
    except ValueError:
        pass
    else:
        raise ConfigurationError("$.updates.host: IP literals are not permitted")
    if mode == "fixture" and host != "fixture.invalid":
        raise ConfigurationError("fixture configuration must use fixture.invalid and cannot contact a real endpoint")
    paths = [updates[name] for name in ("manifest_path", "signature_path", "artifact_path")]
    if any(not re.fullmatch(r"/[A-Za-z0-9._~/-]+", item) or ".." in item for item in paths) or len(set(paths)) != 3:
        raise ConfigurationError("$.updates: paths must be distinct absolute HTTPS paths without traversal")
    if updates["minimum_metadata_version"] != updates["metadata_version"]:
        raise ConfigurationError("$.updates: release metadata version and rollback floor must agree")
    return document


def validate(path: Path, allowed_modes: set[str] | None = None) -> dict:
    return validate_document(load_json(path), allowed_modes)


def canonical_bytes(document: dict) -> bytes:
    return (json.dumps(document, sort_keys=True, separators=(",", ":"), ensure_ascii=True) + "\n").encode("ascii")


def git_bytes(arguments: list[str]) -> bytes:
    result = subprocess.run(["git", *arguments], cwd=ROOT, capture_output=True, check=False)
    if result.returncode:
        raise ConfigurationError(result.stderr.decode("utf-8", "replace").strip() or "git command failed")
    return result.stdout


def source_identity(excluded: tuple[Path, ...] = ()) -> dict[str, object]:
    """Match run_test_evidence.py's named status/diff/untracked identity algorithm."""
    pathspec = ["--", "."]
    for path in excluded:
        try:
            relative = path.resolve().relative_to(ROOT.resolve())
        except ValueError:
            continue
        pathspec.append(f":(exclude){relative.as_posix()}")
    revision = git_bytes(["rev-parse", "HEAD"]).decode("ascii").strip()
    status = git_bytes(["status", "--porcelain=v1", "-z", "--untracked-files=all", *pathspec])
    diff = git_bytes(["diff", "--binary", "--no-ext-diff", "HEAD", *pathspec])
    untracked = git_bytes(["ls-files", "--others", "--exclude-standard", "-z", *pathspec])
    digest = hashlib.sha256()
    for label, value in ((b"status", status), (b"tracked-diff", diff)):
        digest.update(label + b"\0" + len(value).to_bytes(8, "little") + value)
    for raw_path in (entry for entry in untracked.split(b"\0") if entry):
        digest.update(b"untracked\0" + len(raw_path).to_bytes(8, "little") + raw_path)
        with (ROOT / os.fsdecode(raw_path)).open("rb") as stream:
            for block in iter(lambda: stream.read(65536), b""):
                digest.update(len(block).to_bytes(8, "little") + block)
        digest.update((0).to_bytes(8, "little"))
    return {
        "available": True,
        "head": revision,
        "working_tree_dirty": bool(status),
        "source_manifest_sha256": digest.hexdigest(),
    }


def derive_values(document: dict, identity: dict[str, object]) -> dict[str, str]:
    trust, updates, distribution = document["trust"], document["updates"], document["distribution"]
    canonical = canonical_bytes(document)
    return {
        "BARELINE_BUILD_MODE": document["mode"],
        "BARELINE_CONFIG_VERSION": str(document["configuration_version"]),
        "BARELINE_CONFIG_DIGEST": hashlib.sha256(canonical).hexdigest(),
        "BARELINE_SOURCE_REVISION": str(identity["head"]),
        "BARELINE_SOURCE_DIGEST": str(identity["source_manifest_sha256"]),
        "BARELINE_RELEASE_VERSION": distribution["version"],
        "BARELINE_RELEASE_CHANNEL": distribution["channel"],
        "BARELINE_RELEASE_PUBLIC_KEY": trust["release_public_key"],
        "BARELINE_CATALOG_PUBLIC_KEY": trust["catalog_public_key"],
        "BARELINE_PUBLISHER": trust["publisher"],
        "BARELINE_PUBLISHER_CERT_SHA256": trust["publisher_certificate_sha256"],
        "BARELINE_METADATA_FLOOR": str(updates["minimum_metadata_version"]),
        "BARELINE_UPDATE_HOST": updates["host"],
        "BARELINE_UPDATE_MANIFEST_PATH": updates["manifest_path"],
        "BARELINE_UPDATE_SIGNATURE_PATH": updates["signature_path"],
        "BARELINE_UPDATE_ARTIFACT_PATH": updates["artifact_path"],
        "BARELINE_BUILD_FEATURES": "updates=configured,extensions=configured,runtime=external",
        "BARELINE_CONFIG_JSON_B64": base64.b64encode(canonical).decode("ascii"),
        "BARELINE_SOURCE_IDENTITY_ALGORITHM": SOURCE_IDENTITY_ALGORITHM,
        "BARELINE_SOURCE_WORKING_TREE_DIRTY": str(bool(identity["working_tree_dirty"])).lower(),
    }


def prepare(config_path: Path, output: Path, allowed_mode: str) -> None:
    document = validate(config_path, {allowed_mode})
    if document["mode"] == "preview":
        raise ConfigurationError("preview is the default build and does not create trusted compile inputs")
    identity = source_identity((output,))
    values = derive_values(document, identity)
    output.parent.mkdir(parents=True, exist_ok=True)
    if output.exists():
        raise ConfigurationError(f"refusing to overwrite prepared configuration: {output}")
    output.write_bytes(serialize_prepared(values))
    print(f"prepared {document['mode']} configuration {values['BARELINE_CONFIG_DIGEST']} at {output}")


def parse_prepared(path: Path) -> dict[str, str]:
    raw = path.read_bytes()
    if len(raw) > 128 * 1024:
        raise ConfigurationError("prepared configuration exceeds 128 KiB")
    lines = raw.decode("utf-8").splitlines()
    if not lines or lines.pop(0) != "format=bareline-release-prepared-v1":
        raise ConfigurationError("invalid prepared configuration format")
    values: dict[str, str] = {}
    for line in lines:
        key, separator, value = line.partition("=")
        if not separator or key not in PREPARED_KEYS or not value or key in values:
            raise ConfigurationError("invalid prepared configuration entry")
        values[key] = value
    if set(values) != set(PREPARED_KEYS):
        raise ConfigurationError("prepared configuration is incomplete")
    return values


def serialize_prepared(values: dict[str, str]) -> bytes:
    lines = ["format=bareline-release-prepared-v1"] + [f"{key}={values[key]}" for key in PREPARED_KEYS]
    return ("\n".join(lines) + "\n").encode("utf-8")


def verify_prepared(path: Path, allowed_mode: str | None = None) -> dict[str, str]:
    values = parse_prepared(path)
    try:
        canonical = base64.b64decode(values["BARELINE_CONFIG_JSON_B64"], validate=True)
    except ValueError as error:
        raise ConfigurationError("prepared configuration contains invalid canonical JSON encoding") from error
    document = validate_document(load_json_bytes(canonical, "prepared canonical configuration"), {allowed_mode} if allowed_mode else None)
    if canonical != canonical_bytes(document):
        raise ConfigurationError("prepared canonical configuration is not canonical")
    identity = source_identity((path,))
    expected = derive_values(document, identity)
    if values != expected:
        mismatches = sorted(key for key in PREPARED_KEYS if values.get(key) != expected.get(key))
        raise ConfigurationError(f"prepared configuration does not match config/source derivation: {', '.join(mismatches)}")
    return values


def artifact_map(arguments: list[str]) -> dict[str, Path]:
    result: dict[str, Path] = {}
    for argument in arguments:
        role, separator, value = argument.partition("=")
        if not separator or not re.fullmatch(r"[a-z][a-z0-9_-]*", role) or role in result:
            raise ConfigurationError(f"invalid artifact mapping: {argument}")
        path = Path(value).resolve()
        if not path.is_file() or path.is_symlink():
            raise ConfigurationError(f"artifact must be a regular non-link file: {path}")
        result[role] = path
    return result


def sha256_file(path: Path) -> tuple[str, int]:
    digest, size = hashlib.sha256(), 0
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(65536), b""):
            digest.update(chunk); size += len(chunk)
    return digest.hexdigest(), size


def contains_bytes(path: Path, needle: bytes) -> bool:
    carry = b""
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(65536), b""):
            block = carry + chunk
            if needle in block:
                return True
            carry = block[-max(0, len(needle) - 1):]
    return False


def write_json_new(path: Path, document: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("x", encoding="utf-8", newline="\n") as stream:
        json.dump(document, stream, indent=2, sort_keys=True)
        stream.write("\n")


def manifest(prepared: Path, artifacts_arg: list[str], output: Path) -> None:
    values = verify_prepared(prepared)
    artifacts = artifact_map(artifacts_arg)
    required = {"editor", "helper", "runtime"}
    if set(artifacts) != required:
        raise ConfigurationError(f"capability manifest requires exactly these artifacts: {sorted(required)}")
    records = {}
    components = {"editor": "editor", "helper": "update-helper", "runtime": "extension-runtime"}
    for role, path in sorted(artifacts.items()):
        digest, size = sha256_file(path)
        component = components.get(role, role)
        assertion = (
            f"BARELINE-CAPABILITY|component={component}|mode={values['BARELINE_BUILD_MODE']}|"
            f"config-version={values['BARELINE_CONFIG_VERSION']}|config={values['BARELINE_CONFIG_DIGEST']}|"
            f"source={values['BARELINE_SOURCE_DIGEST']}|version={values['BARELINE_RELEASE_VERSION']}|"
            f"features={values['BARELINE_BUILD_FEATURES']}"
        ).encode("ascii")
        if not contains_bytes(path, assertion):
            raise ConfigurationError(f"{role} does not contain the configured capability assertion")
        records[role] = {"file": path.name, "sha256": digest, "bytes": size}
    document = {
        "schema_version": 1, "configuration_version": int(values["BARELINE_CONFIG_VERSION"]),
        "build_mode": values["BARELINE_BUILD_MODE"], "configuration_sha256": values["BARELINE_CONFIG_DIGEST"],
        "source_revision": values["BARELINE_SOURCE_REVISION"], "source_sha256": values["BARELINE_SOURCE_DIGEST"],
        "source_identity_algorithm": values["BARELINE_SOURCE_IDENTITY_ALGORITHM"],
        "source_working_tree_dirty": values["BARELINE_SOURCE_WORKING_TREE_DIRTY"] == "true",
        "distribution_version": values["BARELINE_RELEASE_VERSION"], "channel": values["BARELINE_RELEASE_CHANNEL"],
        "features": values["BARELINE_BUILD_FEATURES"], "runtime_packaging": "separate",
        "artifacts": records,
    }
    write_json_new(output, document)


def inventory(prepared: Path, kind: str, artifacts_arg: list[str], output: Path) -> None:
    values = verify_prepared(prepared)
    artifacts = artifact_map(artifacts_arg)
    required = {
        "core": {"editor", "helper"}, "runtime": {"host"}, "catalog": {"catalog"},
        "components": {"json-tools", "xml-tools", "hex-view"},
    }[kind]
    allowed = required | ({"signature"} if kind == "catalog" else set())
    if not required <= set(artifacts) or not set(artifacts) <= allowed:
        raise ConfigurationError(f"{kind} inventory requires exactly these roles: {sorted(required)}")
    records = []
    for role, path in sorted(artifacts.items()):
        digest, size = sha256_file(path)
        records.append({"role": role, "file": path.name, "sha256": digest, "bytes": size})
    write_json_new(output, {
        "schema_version": 1, "inventory": kind, "signed": False,
        "configuration_version": int(values["BARELINE_CONFIG_VERSION"]),
        "configuration_sha256": values["BARELINE_CONFIG_DIGEST"],
        "source_revision": values["BARELINE_SOURCE_REVISION"],
        "source_sha256": values["BARELINE_SOURCE_DIGEST"],
        "source_identity_algorithm": values["BARELINE_SOURCE_IDENTITY_ALGORITHM"],
        "source_working_tree_dirty": values["BARELINE_SOURCE_WORKING_TREE_DIRTY"] == "true",
        "distribution_version": values["BARELINE_RELEASE_VERSION"],
        "build_mode": values["BARELINE_BUILD_MODE"], "channel": values["BARELINE_RELEASE_CHANNEL"],
        "features": values["BARELINE_BUILD_FEATURES"], "artifacts": records,
    })


def verify_manifest(path: Path, artifact_dir: Path) -> None:
    document = load_json(path)
    required_fields = {
        "schema_version", "configuration_version", "build_mode", "configuration_sha256",
        "source_revision", "source_sha256", "source_identity_algorithm", "source_working_tree_dirty",
        "distribution_version", "channel", "features",
        "runtime_packaging", "artifacts",
    }
    if not isinstance(document, dict) or set(document) != required_fields or document.get("schema_version") != 1:
        raise ConfigurationError("capability manifest has an incomplete or unknown top-level field set")
    if not isinstance(document["configuration_version"], int) or document["configuration_version"] < 1:
        raise ConfigurationError("invalid capability configuration version")
    if document["build_mode"] not in ("configured", "fixture"):
        raise ConfigurationError("invalid capability build mode")
    for field in ("configuration_sha256", "source_sha256"):
        if not isinstance(document[field], str) or re.fullmatch(r"[0-9a-f]{64}", document[field]) is None:
            raise ConfigurationError(f"invalid capability {field}")
    if not isinstance(document["source_revision"], str) or re.fullmatch(r"[0-9a-f]{40}", document["source_revision"]) is None:
        raise ConfigurationError("invalid capability source revision")
    if document["source_identity_algorithm"] != SOURCE_IDENTITY_ALGORITHM or not isinstance(document["source_working_tree_dirty"], bool):
        raise ConfigurationError("invalid capability source identity type")
    if not isinstance(document["distribution_version"], str) or re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-(?:beta|rc)\.[1-9][0-9]*)?", document["distribution_version"]) is None:
        raise ConfigurationError("invalid capability distribution version")
    if document["channel"] not in ("stable", "beta") or document["runtime_packaging"] != "separate":
        raise ConfigurationError("invalid capability channel/runtime packaging")
    expected_features = "updates=configured,extensions=configured,runtime=external"
    if document["features"] != expected_features:
        raise ConfigurationError("invalid capability feature assertion")
    records = document["artifacts"]
    expected_files = {"editor": "bareline.exe", "helper": "bareline-update-helper.exe", "runtime": "bareline-extension-host.exe"}
    components = {"editor": "editor", "helper": "update-helper", "runtime": "extension-runtime"}
    if not isinstance(records, dict) or set(records) != set(expected_files):
        raise ConfigurationError("capability manifest requires exactly editor/helper/runtime roles")
    for role, record in records.items():
        if not isinstance(record, dict) or set(record) != {"file", "sha256", "bytes"}:
            raise ConfigurationError(f"{role}: invalid artifact record")
        if record["file"] != expected_files[role] or not isinstance(record["bytes"], int) or record["bytes"] <= 0:
            raise ConfigurationError(f"{role}: invalid artifact filename or size")
        if not isinstance(record["sha256"], str) or re.fullmatch(r"[0-9a-f]{64}", record["sha256"]) is None:
            raise ConfigurationError(f"{role}: invalid artifact digest")
        candidate = (artifact_dir / record["file"]).resolve()
        if candidate.parent != artifact_dir.resolve() or not candidate.is_file():
            raise ConfigurationError(f"{role}: unsafe or missing artifact")
        digest, size = sha256_file(candidate)
        if digest != record["sha256"] or size != record["bytes"]:
            raise ConfigurationError(f"{role}: artifact bytes differ from manifest")
        assertion = (
            f"BARELINE-CAPABILITY|component={components[role]}|mode={document['build_mode']}|"
            f"config-version={document['configuration_version']}|config={document['configuration_sha256']}|"
            f"source={document['source_sha256']}|version={document['distribution_version']}|"
            f"features={document['features']}"
        ).encode("ascii")
        if not contains_bytes(candidate, assertion):
            raise ConfigurationError(f"{role}: embedded capability assertion disagrees with manifest")
    print("PASS: capability manifest matches exact unsigned artifact bytes")


def verify_inventory(path: Path, values: dict[str, str]) -> dict:
    document = load_json(path)
    required_fields = {
        "schema_version", "inventory", "signed", "configuration_version",
        "configuration_sha256", "source_revision", "source_sha256", "source_identity_algorithm",
        "source_working_tree_dirty", "distribution_version", "build_mode", "channel", "features", "artifacts",
    }
    claims = {
        "configuration_version": int(values["BARELINE_CONFIG_VERSION"]),
        "configuration_sha256": values["BARELINE_CONFIG_DIGEST"],
        "source_revision": values["BARELINE_SOURCE_REVISION"],
        "source_sha256": values["BARELINE_SOURCE_DIGEST"],
        "source_identity_algorithm": values["BARELINE_SOURCE_IDENTITY_ALGORITHM"],
        "source_working_tree_dirty": values["BARELINE_SOURCE_WORKING_TREE_DIRTY"] == "true",
        "distribution_version": values["BARELINE_RELEASE_VERSION"],
        "build_mode": values["BARELINE_BUILD_MODE"], "channel": values["BARELINE_RELEASE_CHANNEL"],
        "features": values["BARELINE_BUILD_FEATURES"],
    }
    if (
        not isinstance(document, dict)
        or set(document) != required_fields
        or document.get("schema_version") != 1
        or document.get("signed") is not False
        or document.get("inventory") not in {"core", "runtime", "catalog", "components"}
    ):
        raise ConfigurationError(f"{path}: invalid inventory header")
    if any(document.get(key) != value for key, value in claims.items()):
        raise ConfigurationError(f"{path}: inventory provenance does not match prepared config/source")
    records = document.get("artifacts")
    if not isinstance(records, list) or not records:
        raise ConfigurationError(f"{path}: inventory has no artifacts")
    observed = set()
    for record in records:
        if not isinstance(record, dict) or set(record) != {"role", "file", "sha256", "bytes"} or record["role"] in observed:
            raise ConfigurationError(f"{path}: malformed or duplicate inventory record")
        observed.add(record["role"])
        candidate = (path.parent / record["file"]).resolve()
        if candidate.parent != path.parent.resolve() or not candidate.is_file() or candidate.is_symlink():
            raise ConfigurationError(f"{path}: unsafe inventory artifact path")
        digest, size = sha256_file(candidate)
        if digest != record["sha256"] or size != record["bytes"]:
            raise ConfigurationError(f"{path}: inventory artifact bytes changed")
    required_roles = {
        "core": {"editor", "helper"},
        "runtime": {"host"},
        "catalog": {"catalog"},
        "components": {"json-tools", "xml-tools", "hex-view"},
    }[document["inventory"]]
    allowed_roles = required_roles | ({"signature"} if document["inventory"] == "catalog" else set())
    if not required_roles <= observed or not observed <= allowed_roles:
        raise ConfigurationError(f"{path}: inventory roles disagree with kind")
    return document


def verify_t10_source_receipt(t10: object, values: dict[str, str]) -> None:
    if not isinstance(t10, dict) or t10.get("status") != "passed" or t10.get("source_identity_chain_verified") is not True:
        raise ConfigurationError("T10 fixture dependency lacks passed execution and source continuity")

    def matches(identity: object) -> bool:
        return (
            isinstance(identity, dict)
            and identity.get("available") is True
            and identity.get("head") == values["BARELINE_SOURCE_REVISION"]
            and identity.get("working_tree_dirty") is (values["BARELINE_SOURCE_WORKING_TREE_DIRTY"] == "true")
            and identity.get("source_manifest_sha256") == values["BARELINE_SOURCE_DIGEST"]
        )

    if not matches(t10.get("source_identity")) or not matches(t10.get("source_identity_after")):
        raise ConfigurationError("T10 execution source identity does not match the prepared release source")


def _comparable_local_path(path: Path) -> str:
    value = str(path)
    if os.name == "nt" and value.startswith("\\\\?\\UNC\\"):
        value = "\\\\" + value[8:]
    elif os.name == "nt" and value.startswith("\\\\?\\"):
        if not re.fullmatch(r"\\\\\?\\[A-Za-z]:\\.*", value):
            raise ConfigurationError("unsupported Windows device path")
        value = value[4:]
    if os.name == "nt" and value.startswith("\\\\.\\"):
        raise ConfigurationError("unsupported Windows device path")
    if not os.path.isabs(value):
        raise ConfigurationError("artifact path is not absolute")
    return os.path.normcase(os.path.normpath(value))


def verified_installed_artifact_path(path_text: str, installation_root: Path) -> Path:
    if not path_text or "\0" in path_text:
        raise ConfigurationError("artifact path is empty or contains NUL")
    candidate = Path(os.path.normpath(path_text))
    root = installation_root.resolve(strict=True)
    candidate_key = _comparable_local_path(candidate)
    root_key = _comparable_local_path(root)
    try:
        if os.path.commonpath((root_key, candidate_key)) != root_key or candidate_key == root_key:
            raise ConfigurationError("artifact path escapes the installation root")
    except ValueError as error:
        raise ConfigurationError("artifact path is on a different volume") from error

    reached_root = False
    for current in (candidate, *candidate.parents):
        try:
            metadata = current.lstat()
        except OSError as error:
            raise ConfigurationError("artifact path is missing") from error
        attributes = getattr(metadata, "st_file_attributes", 0)
        if stat.S_ISLNK(metadata.st_mode) or attributes & getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0):
            raise ConfigurationError("artifact path traverses a reparse point")
        if _comparable_local_path(current) == root_key:
            reached_root = True
            break
    if not reached_root or not stat.S_ISREG(candidate.lstat().st_mode):
        raise ConfigurationError("artifact path is not a regular file below the installation root")

    resolved = candidate.resolve(strict=True)
    resolved_key = _comparable_local_path(resolved)
    try:
        if os.path.commonpath((root_key, resolved_key)) != root_key or resolved_key == root_key:
            raise ConfigurationError("resolved artifact path escapes the installation root")
    except ValueError as error:
        raise ConfigurationError("resolved artifact path is on a different volume") from error
    return candidate


def fixture_report(
    prepared: Path,
    t10_path: Path,
    installed_path: Path,
    capability: Path,
    inventories: list[Path],
    output: Path,
) -> None:
    values = verify_prepared(prepared, "fixture")
    t10 = load_json(t10_path)
    verify_t10_source_receipt(t10, values)
    assert isinstance(t10, dict)
    installed = load_json(installed_path)
    if not isinstance(installed, dict) or set(installed) != {"schema_version", "nonshipping", "artifacts"} or installed.get("schema_version") != 1 or installed.get("nonshipping") is not True:
        raise ConfigurationError("installed fixture manifest has an invalid boundary")
    installed_artifacts = installed.get("artifacts")
    if not isinstance(installed_artifacts, dict) or set(installed_artifacts) != {"host", "json", "xml", "hex"}:
        raise ConfigurationError("installed fixture manifest requires exactly host/json/xml/hex")
    installation_root = installed_path.parent / "installation"
    for role, record in installed_artifacts.items():
        if not isinstance(record, dict) or set(record) != {"path", "sha256", "bytes"} or not isinstance(record["path"], str):
            raise ConfigurationError(f"installed fixture {role} identity is malformed")
        try:
            candidate = verified_installed_artifact_path(record["path"], installation_root)
        except ConfigurationError:
            raise ConfigurationError(f"installed fixture {role} path is unsafe or missing")
        digest, size = sha256_file(candidate)
        if digest != record["sha256"] or size != record["bytes"]:
            raise ConfigurationError(f"installed fixture {role} bytes changed")
    artifacts = t10.get("artifacts")
    if not isinstance(artifacts, dict):
        raise ConfigurationError("T10 fixture dependency lacks artifacts")
    artifact_inputs = t10.get("artifact_inputs")
    if not isinstance(artifact_inputs, dict) or set(artifact_inputs) != {"mode", "validated_before_execution"} or artifact_inputs.get("mode") != "explicit":
        raise ConfigurationError("T10 fixture dependency did not use explicit installed artifact inputs")
    validated_inputs = artifact_inputs.get("validated_before_execution")
    if not isinstance(validated_inputs, dict) or set(validated_inputs) != {"host", "json", "xml", "hex"}:
        raise ConfigurationError("T10 fixture dependency lacks pre-execution artifact identities")
    observed = t10.get("observed_suites")
    if not isinstance(observed, list) or len(observed) != 1 or not isinstance(observed[0], dict) or any(observed[0].get(key) != value for key, value in {"suite": "fast", "expected": 3, "passed": 3, "failed": 0}.items()):
        raise ConfigurationError("T10 fixture dependency requires one three-case Fast execution")
    for role in ("host", "json", "xml", "hex"):
        record = artifacts.get(role)
        if not isinstance(record, dict) or not isinstance(record.get("path"), str):
            raise ConfigurationError(f"T10 fixture dependency lacks explicit {role} identity")
        path = Path(record["path"])
        digest, size = sha256_file(path)
        if digest != record.get("sha256") or size != record.get("bytes"):
            raise ConfigurationError(f"T10 {role} component changed after execution")
        expected = installed_artifacts[role]
        try:
            same_file = os.path.samefile(path, expected["path"])
        except OSError:
            same_file = False
        if not same_file or digest != expected["sha256"] or size != expected["bytes"]:
            raise ConfigurationError(f"T10 {role} input was not the retained fixture installation")
        if validated_inputs[role] != record:
            raise ConfigurationError(f"T10 {role} post-execution identity differs from its validated input")
    verify_manifest(capability, capability.parent / "unsigned-executables" if (capability.parent / "unsigned-executables").is_dir() else ROOT / "target" / "debug")
    capability_document = load_json(capability)
    assert isinstance(capability_document, dict)
    capability_claims = {
        "configuration_version": int(values["BARELINE_CONFIG_VERSION"]),
        "configuration_sha256": values["BARELINE_CONFIG_DIGEST"],
        "source_revision": values["BARELINE_SOURCE_REVISION"],
        "source_sha256": values["BARELINE_SOURCE_DIGEST"],
        "source_identity_algorithm": values["BARELINE_SOURCE_IDENTITY_ALGORITHM"],
        "source_working_tree_dirty": values["BARELINE_SOURCE_WORKING_TREE_DIRTY"] == "true",
        "distribution_version": values["BARELINE_RELEASE_VERSION"],
        "build_mode": values["BARELINE_BUILD_MODE"],
        "channel": values["BARELINE_RELEASE_CHANNEL"],
        "features": values["BARELINE_BUILD_FEATURES"],
    }
    if any(capability_document.get(key) != value for key, value in capability_claims.items()):
        raise ConfigurationError("capability manifest provenance does not match the prepared config/source")
    if len(inventories) != 3:
        raise ConfigurationError("fixture report requires runtime, catalog, and component inventories")
    inventory_documents = {}
    for path in inventories:
        document = verify_inventory(path, values)
        kind = document["inventory"]
        if kind in inventory_documents:
            raise ConfigurationError(f"duplicate {kind} inventory")
        inventory_documents[kind] = (path, document)
    if set(inventory_documents) != {"runtime", "catalog", "components"}:
        raise ConfigurationError("fixture report requires exactly runtime, catalog, and component inventories")
    catalog_records = {record["role"]: record for record in inventory_documents["catalog"][1]["artifacts"]}
    if "signature" not in catalog_records:
        raise ConfigurationError("fixture catalog inventory must retain its detached signature")
    runtime_records = {record["role"]: record for record in inventory_documents["runtime"][1]["artifacts"]}
    if runtime_records["host"]["sha256"] != capability_document["artifacts"]["runtime"]["sha256"]:
        raise ConfigurationError("inventoried runtime differs from the capability-manifest runtime")
    if runtime_records["host"]["sha256"] != installed_artifacts["host"]["sha256"]:
        raise ConfigurationError("inventoried runtime differs from the installed and executed runtime")
    component_path, component_document = inventory_documents["components"]
    component_records = {record["role"]: record for record in component_document["artifacts"]}
    t10_roles = {"json-tools": "json", "xml-tools": "xml", "hex-view": "hex"}
    for package_role, t10_role in t10_roles.items():
        package = component_path.parent / component_records[package_role]["file"]
        try:
            with zipfile.ZipFile(package) as archive:
                names = archive.namelist()
                if names.count("extension.wasm") != 1:
                    raise ConfigurationError(f"{package}: package does not contain one extension.wasm")
                component_bytes = archive.read("extension.wasm")
        except (zipfile.BadZipFile, KeyError) as error:
            raise ConfigurationError(f"{package}: invalid component package: {error}") from error
        if hashlib.sha256(component_bytes).hexdigest() != artifacts[t10_role]["sha256"] or len(component_bytes) != artifacts[t10_role]["bytes"]:
            raise ConfigurationError(f"{package_role}: packaged component differs from the T10-executed component")
    references = {}
    for name, path in [("t10_execution", t10_path), ("installed_artifacts", installed_path), ("capabilities", capability), *[(f"inventory_{index}", item) for index, item in enumerate(inventories)]]:
        digest, size = sha256_file(path)
        references[name] = {"file": str(path.resolve()), "sha256": digest, "bytes": size}
    write_json_new(output, {
        "schema_version": 1, "status": "fixture-qualified", "nonshipping": True,
        "configuration_sha256": values["BARELINE_CONFIG_DIGEST"], "source_revision": values["BARELINE_SOURCE_REVISION"],
        "source_sha256": values["BARELINE_SOURCE_DIGEST"],
        "source_identity_algorithm": values["BARELINE_SOURCE_IDENTITY_ALGORITHM"],
        "source_working_tree_dirty": values["BARELINE_SOURCE_WORKING_TREE_DIRTY"] == "true",
        "distribution_version": values["BARELINE_RELEASE_VERSION"],
        "references": references,
    })


def main() -> int:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)
    validate_parser = sub.add_parser("validate"); validate_parser.add_argument("--config", type=Path, required=True)
    prepare_parser = sub.add_parser("prepare"); prepare_parser.add_argument("--config", type=Path, required=True); prepare_parser.add_argument("--output", type=Path, required=True); prepare_parser.add_argument("--mode", choices=("configured", "fixture"), default="configured")
    prepared_parser = sub.add_parser("verify-prepared"); prepared_parser.add_argument("--prepared", type=Path, required=True); prepared_parser.add_argument("--mode", choices=("configured", "fixture")); prepared_parser.add_argument("--emit-receipt", action="store_true")
    manifest_parser = sub.add_parser("manifest"); manifest_parser.add_argument("--prepared", type=Path, required=True); manifest_parser.add_argument("--artifact", action="append", default=[]); manifest_parser.add_argument("--output", type=Path, required=True)
    inventory_parser = sub.add_parser("inventory"); inventory_parser.add_argument("--prepared", type=Path, required=True); inventory_parser.add_argument("--kind", choices=("core", "runtime", "catalog", "components"), required=True); inventory_parser.add_argument("--artifact", action="append", default=[]); inventory_parser.add_argument("--output", type=Path, required=True)
    verify_parser = sub.add_parser("verify-manifest"); verify_parser.add_argument("--manifest", type=Path, required=True); verify_parser.add_argument("--artifact-dir", type=Path, required=True)
    report_parser = sub.add_parser("fixture-report"); report_parser.add_argument("--prepared", type=Path, required=True); report_parser.add_argument("--t10-manifest", type=Path, required=True); report_parser.add_argument("--installed-manifest", type=Path, required=True); report_parser.add_argument("--capability-manifest", type=Path, required=True); report_parser.add_argument("--inventory", type=Path, action="append", default=[]); report_parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.command == "validate": validate(args.config); print(f"PASS: {args.config} conforms to release-config-v1")
        elif args.command == "prepare": prepare(args.config, args.output, args.mode)
        elif args.command == "verify-prepared":
            values = verify_prepared(args.prepared, args.mode)
            if args.emit_receipt:
                sys.stdout.buffer.write(serialize_prepared(values))
            else:
                print("PASS: prepared configuration matches canonical config and current source")
        elif args.command == "manifest": manifest(args.prepared, args.artifact, args.output)
        elif args.command == "inventory": inventory(args.prepared, args.kind, args.artifact, args.output)
        elif args.command == "verify-manifest": verify_manifest(args.manifest, args.artifact_dir)
        else: fixture_report(args.prepared, args.t10_manifest, args.installed_manifest, args.capability_manifest, args.inventory, args.output)
    except (ConfigurationError, OSError, subprocess.CalledProcessError) as error:
        print(f"release configuration error: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
