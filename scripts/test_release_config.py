#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0
"""Bounded parser/configuration probes. This script never builds or contacts a host."""

import copy
import base64
import importlib.util
import hashlib
import json
import os
import subprocess
import tempfile
import zipfile
from pathlib import Path

import release_config


def rejected(document: dict, message: str) -> None:
    with tempfile.TemporaryDirectory(prefix="bareline-release-config-") as temporary:
        path = Path(temporary) / "invalid.json"
        path.write_text(json.dumps(document), encoding="utf-8")
        try:
            release_config.validate(path)
        except release_config.ConfigurationError:
            return
        raise AssertionError(message)


def main() -> None:
    if os.name == "nt":
        assert release_config._comparable_local_path(Path(r"\\?\C:\retained\artifact")) == release_config._comparable_local_path(Path(r"C:\retained\artifact"))
        assert release_config._comparable_local_path(Path(r"\\?\UNC\server\share\artifact")) == release_config._comparable_local_path(Path(r"\\server\share\artifact"))
        for device_path in (r"\\.\PhysicalDrive0", r"\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy1"):
            try: release_config._comparable_local_path(Path(device_path))
            except release_config.ConfigurationError: pass
            else: raise AssertionError(f"unsupported device path accepted: {device_path}")
    preview = release_config.validate(release_config.ROOT / "release" / "preview.release-config.json", {"preview"})
    fixture = release_config.validate(
        release_config.ROOT / "release" / "fixtures" / "nonshipping.release-config.json", {"fixture"}
    )
    configured = copy.deepcopy(fixture); configured["mode"] = "configured"; configured["updates"]["host"] = "releases.bareline.app"
    configured["trust"]["release_public_key"] = base64.b64encode(b"EdRELEASE1" + bytes([11]) * 32).decode("ascii")
    configured["trust"]["catalog_public_key"] = base64.b64encode(b"EdCATALOG1" + bytes([12]) * 32).decode("ascii")
    configured["trust"]["offline_root_public_key"] = base64.b64encode(b"EdOFFLINE1" + bytes([14]) * 32).decode("ascii")
    configured["trust"]["publisher_certificate_sha256"] = "1" * 64
    with tempfile.TemporaryDirectory(prefix="bareline-release-config-") as temporary:
        configured_path = Path(temporary) / "configured.json"
        configured_path.write_text(json.dumps(configured), encoding="utf-8")
        release_config.validate(configured_path, {"configured"})
    cases = []
    changed = copy.deepcopy(fixture); changed["unexpected"] = True; cases.append((changed, "unknown key accepted"))
    changed = copy.deepcopy(fixture); changed["trust"]["release_public_key"] = "test-key"; cases.append((changed, "invalid key accepted"))
    changed = copy.deepcopy(fixture); changed["trust"]["catalog_public_key"] = changed["trust"]["release_public_key"]; cases.append((changed, "key reuse accepted"))
    changed = copy.deepcopy(fixture); changed["trust"]["publisher_certificate_sha256"] = "A" * 64; cases.append((changed, "invalid certificate digest accepted"))
    changed = copy.deepcopy(fixture); changed["distribution"]["channel"] = "preview"; cases.append((changed, "wrong channel accepted"))
    changed = copy.deepcopy(fixture); changed["distribution"]["version"] = "0.1.0-beta.1"; cases.append((changed, "channel/package disagreement accepted"))
    changed = copy.deepcopy(fixture); changed["updates"]["minimum_metadata_version"] = 2; cases.append((changed, "stale rollback floor accepted"))
    changed = copy.deepcopy(fixture); changed["updates"]["host"] = "localhost"; cases.append((changed, "fixture real host accepted"))
    changed = copy.deepcopy(preview); changed["trust"]["publisher"] = "fixture"; cases.append((changed, "preview trust accepted"))
    changed = copy.deepcopy(fixture); changed["mode"] = "configured"; changed["updates"]["host"] = "releases.bareline.app"; cases.append((changed, "configured mode accepted checked-in fixture trust"))
    for document, message in cases:
        rejected(document, message)
    with tempfile.TemporaryDirectory(prefix="bareline-release-contract-") as temporary:
        root = Path(temporary)
        config = root / "configured.json"; config.write_text(json.dumps(configured), encoding="utf-8")
        prepared = root / "prepared.txt"; release_config.prepare(config, prepared, "configured")
        values = release_config.verify_prepared(prepared, "configured")
        emitted = subprocess.run(
            ["python", str(release_config.ROOT / "scripts/release_config.py"), "verify-prepared", "--prepared", str(prepared), "--mode", "configured", "--emit-receipt"],
            cwd=release_config.ROOT, capture_output=True, check=True,
        ).stdout
        assert emitted == prepared.read_bytes(), "build-script handoff differs from validated receipt bytes"
        evidence_spec = importlib.util.spec_from_file_location(
            "run_test_evidence", release_config.ROOT / ".github/workflows/run_test_evidence.py"
        )
        assert evidence_spec is not None and evidence_spec.loader is not None
        evidence = importlib.util.module_from_spec(evidence_spec); evidence_spec.loader.exec_module(evidence)
        assert release_config.source_identity() == evidence.source_identity(release_config.ROOT)
        receipt_identity = release_config.source_identity()
        actual_shape_receipt = {
            "status": "passed",
            "source_identity_chain_verified": True,
            "source_identity": receipt_identity,
            "source_identity_after": copy.deepcopy(receipt_identity),
        }
        release_config.verify_t10_source_receipt(actual_shape_receipt, values)
        changed_receipt = copy.deepcopy(actual_shape_receipt)
        changed_receipt["source_identity_after"]["source_manifest_sha256"] = "f" * 64
        try: release_config.verify_t10_source_receipt(changed_receipt, values)
        except release_config.ConfigurationError: pass
        else: raise AssertionError("changed four-field T10 source receipt accepted")
        source_probe = release_config.ROOT / f".release-config-source-probe-{os.getpid()}"
        try:
            source_probe.write_text("source change probe\n", encoding="utf-8")
            try: release_config.verify_prepared(prepared, "configured")
            except release_config.ConfigurationError: pass
            else: raise AssertionError("source change did not invalidate prepared configuration")
        finally:
            source_probe.unlink(missing_ok=True)
        release_config.verify_prepared(prepared, "configured")
        tampered = prepared.read_text(encoding="utf-8").replace("BARELINE_RELEASE_CHANNEL=stable", "BARELINE_RELEASE_CHANNEL=beta")
        tampered_path = root / "tampered.txt"; tampered_path.write_text(tampered, encoding="utf-8")
        try: release_config.verify_prepared(tampered_path, "configured")
        except release_config.ConfigurationError: pass
        else: raise AssertionError("tampered prepared values accepted")

        artifact_dir = root / "artifacts"; artifact_dir.mkdir()
        components = {"editor": ("editor", "bareline.exe"), "helper": ("update-helper", "bareline-update-helper.exe"), "runtime": ("extension-runtime", "bareline-extension-host.exe")}
        artifacts = []
        for role, (component, filename) in components.items():
            marker = (
                f"BARELINE-CAPABILITY|component={component}|mode={values['BARELINE_BUILD_MODE']}|"
                f"config-version={values['BARELINE_CONFIG_VERSION']}|config={values['BARELINE_CONFIG_DIGEST']}|"
                f"source={values['BARELINE_SOURCE_DIGEST']}|version={values['BARELINE_RELEASE_VERSION']}|"
                f"features={values['BARELINE_BUILD_FEATURES']}"
            )
            path = artifact_dir / filename; path.write_bytes(("fixture-binary\0" + marker).encode("ascii")); artifacts.append(f"{role}={path}")
        manifest = root / "manifest.json"; release_config.manifest(prepared, artifacts, manifest)
        release_config.verify_manifest(manifest, artifact_dir)
        original = json.loads(manifest.read_text(encoding="utf-8"))
        for name, mutate in (
            ("missing-role", lambda value: value["artifacts"].pop("runtime")),
            ("changed-claim", lambda value: value.__setitem__("source_sha256", "f" * 64)),
            ("malformed-path", lambda value: value["artifacts"]["editor"].__setitem__("file", "../bareline.exe")),
        ):
            changed = copy.deepcopy(original); mutate(changed)
            changed_path = root / f"{name}.json"; changed_path.write_text(json.dumps(changed), encoding="utf-8")
            try: release_config.verify_manifest(changed_path, artifact_dir)
            except release_config.ConfigurationError: pass
            else: raise AssertionError(f"manifest {name} accepted")
        editor = artifact_dir / "bareline.exe"; editor.write_bytes(editor.read_bytes() + b"tamper")
        try: release_config.verify_manifest(manifest, artifact_dir)
        except release_config.ConfigurationError: pass
        else: raise AssertionError("tampered artifact bytes accepted")
    with tempfile.TemporaryDirectory(prefix="bareline-release-report-") as temporary:
        root = Path(temporary)
        fixture_path = root / "fixture.json"; fixture_path.write_text(json.dumps(fixture), encoding="utf-8")
        prepared = root / "prepared.txt"; release_config.prepare(fixture_path, prepared, "fixture")
        values = release_config.verify_prepared(prepared, "fixture")
        executables = root / "unsigned-executables"; executables.mkdir()
        component_names = {"editor": "editor", "helper": "update-helper", "runtime": "extension-runtime"}
        artifact_arguments = []
        for role, component in component_names.items():
            filename = {"editor": "bareline.exe", "helper": "bareline-update-helper.exe", "runtime": "bareline-extension-host.exe"}[role]
            marker = (
                f"BARELINE-CAPABILITY|component={component}|mode=fixture|config-version={values['BARELINE_CONFIG_VERSION']}|"
                f"config={values['BARELINE_CONFIG_DIGEST']}|source={values['BARELINE_SOURCE_DIGEST']}|"
                f"version={values['BARELINE_RELEASE_VERSION']}|features={values['BARELINE_BUILD_FEATURES']}"
            ).encode("ascii")
            path = executables / filename; path.write_bytes(b"MZ-static-fixture\0" + marker); artifact_arguments.append(f"{role}={path}")
        capability = root / "build-capabilities.json"; release_config.manifest(prepared, artifact_arguments, capability)
        delivery = root / "connected-delivery"; installation = delivery / "installation"; installation.mkdir(parents=True)
        long_segment = "retained-" + "a" * 64
        def retained_path(relative: str) -> Path:
            path = installation / long_segment / long_segment / relative
            if os.name == "nt":
                path = Path("\\\\?\\" + str(path.resolve()))
            return path
        installed_paths = {
            "host": retained_path("runtimes/hash/bareline-extension-host.exe"),
            "json": retained_path("json/extension.wasm"),
            "xml": retained_path("xml/extension.wasm"),
            "hex": retained_path("hex/extension.wasm"),
        }
        installed_paths["host"].parent.mkdir(parents=True); installed_paths["host"].write_bytes((executables / "bareline-extension-host.exe").read_bytes())
        for role in ("json", "xml", "hex"):
            installed_paths[role].parent.mkdir(parents=True); installed_paths[role].write_bytes(b"\0asm\x0d\0\x01\0" + role.encode("ascii"))
        def identity(path: Path) -> dict:
            payload = path.read_bytes()
            return {"path": str(path.resolve()), "sha256": hashlib.sha256(payload).hexdigest(), "bytes": len(payload)}
        installed_artifacts = {role: identity(path) for role, path in installed_paths.items()}
        installed_manifest = delivery / "installed-artifacts.json"
        installed_manifest.write_text(json.dumps({"schema_version": 1, "nonshipping": True, "artifacts": installed_artifacts}), encoding="utf-8")
        assert all(len(record["path"]) > 260 for record in installed_artifacts.values())
        for record in installed_artifacts.values():
            release_config.verified_installed_artifact_path(record["path"], installation)
        outside = root / "outside.wasm"; outside.write_bytes(b"outside")
        for unsafe in (str(outside.resolve()), str((installation / "missing.wasm").resolve())):
            try: release_config.verified_installed_artifact_path(unsafe, installation)
            except release_config.ConfigurationError: pass
            else: raise AssertionError(f"unsafe or missing installed path accepted: {unsafe}")
        assets = root / "delivery-assets"; assets.mkdir()
        (assets / "bareline-exthost-x64.exe").write_bytes(installed_paths["host"].read_bytes())
        (assets / "catalog.json").write_text("{}", encoding="ascii"); (assets / "catalog.minisig").write_text("fixture", encoding="ascii")
        package_roles = {"json-tools": "json", "xml-tools": "xml", "hex-view": "hex"}
        for package_role, role in package_roles.items():
            with zipfile.ZipFile(assets / f"{package_role}.blex", "w") as archive:
                archive.writestr("extension.wasm", installed_paths[role].read_bytes())
        runtime_inventory = assets / "runtime-inventory.json"
        catalog_inventory = assets / "catalog-inventory.json"
        component_inventory = assets / "components-inventory.json"
        release_config.inventory(prepared, "runtime", [f"host={assets / 'bareline-exthost-x64.exe'}"], runtime_inventory)
        release_config.inventory(prepared, "catalog", [f"catalog={assets / 'catalog.json'}", f"signature={assets / 'catalog.minisig'}"], catalog_inventory)
        release_config.inventory(prepared, "components", [f"{role}={assets / (role + '.blex')}" for role in package_roles], component_inventory)
        source = release_config.source_identity()
        t10 = {
            "status": "passed", "source_identity_chain_verified": True,
            "source_identity": source, "source_identity_after": copy.deepcopy(source),
            "observed_suites": [{"suite": "fast", "expected": 3, "passed": 3, "failed": 0}],
            "artifact_inputs": {"mode": "explicit", "validated_before_execution": installed_artifacts},
            "artifacts": copy.deepcopy(installed_artifacts),
        }
        t10_manifest = root / "t10-run.json"; t10_manifest.write_text(json.dumps(t10), encoding="utf-8")
        report = root / "fixture-report.json"
        release_config.fixture_report(prepared, t10_manifest, installed_manifest, capability, [runtime_inventory, catalog_inventory, component_inventory], report)
        assert report.is_file()
        changed_t10 = copy.deepcopy(t10); changed_t10["artifact_inputs"]["mode"] = "default"
        changed_t10_path = root / "changed-t10.json"; changed_t10_path.write_text(json.dumps(changed_t10), encoding="utf-8")
        try: release_config.fixture_report(prepared, changed_t10_path, installed_manifest, capability, [runtime_inventory, catalog_inventory, component_inventory], root / "rejected-report.json")
        except release_config.ConfigurationError: pass
        else: raise AssertionError("fixture report accepted non-explicit T10 artifacts")
    duplicate = '{"schema_version":1,"schema_version":1}'
    with tempfile.TemporaryDirectory(prefix="bareline-release-config-") as temporary:
        duplicate_path = Path(temporary) / "duplicate.json"; duplicate_path.write_text(duplicate, encoding="utf-8")
        try: release_config.load_json(duplicate_path)
        except release_config.ConfigurationError: pass
        else: raise AssertionError("duplicate JSON key accepted")
    print(f"PASS: modes, {len(cases)} invalid configs, prepared/source/receipt binding, strict manifests, linked fixture-report parser probe, duplicate keys")


if __name__ == "__main__":
    main()
