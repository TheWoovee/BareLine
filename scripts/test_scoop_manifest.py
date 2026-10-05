# SPDX-License-Identifier: MPL-2.0
"""Scoop manifest template oracles on synthetic release directories; nothing is downloaded."""
import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import scoop_manifest as scoop

TAG = "v0.2.0-preview.1"
ZIP = "bareline-0.2.0-windows-x64-portable.zip"


class ScoopManifestTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="bareline-scoop-")
        self.root = Path(self.temporary.name)
        self.artifacts = self.root / "preview"
        self.artifacts.mkdir()
        (self.artifacts / ZIP).write_bytes(b"synthetic portable zip")
        self.digest = hashlib.sha256(b"synthetic portable zip").hexdigest()
        self.write_sums({ZIP: self.digest, "LICENSE": "0" * 64})

    def tearDown(self):
        self.temporary.cleanup()

    def write_sums(self, entries):
        lines = [f"{digest}  {name}" for name, digest in sorted(entries.items())]
        (self.artifacts / "SHA-256SUMS").write_text("\n".join(lines) + "\n", encoding="utf-8")

    def template(self, change):
        manifest = json.loads(scoop.TEMPLATE.read_text(encoding="utf-8"))
        change(manifest)
        path = self.root / "template.json"
        path.write_text(json.dumps(manifest), encoding="utf-8")
        return path

    def test_checked_in_template_follows_a_new_preview_tag(self):
        manifest = scoop.render(scoop.TEMPLATE, self.artifacts, TAG, "0.2.0")
        self.assertEqual(manifest["version"], "0.2.0-preview.1")
        self.assertEqual(manifest["architecture"]["64bit"], {
            "url": f"https://github.com/TheWoovee/BareLine/releases/download/{TAG}/{ZIP}", "hash": self.digest})
        self.assertIn("autoupdate", manifest)
        self.assertEqual(manifest["bin"], "bareline.exe")

    def test_checked_in_release_matches_its_own_autoupdate_rules(self):
        # The checked-in manifest records the last published preview, which can be older than the Cargo version.
        manifest = scoop.read_manifest(scoop.TEMPLATE)
        core = manifest["version"].split("-", 1)[0]
        self.assertTrue(manifest["architecture"]["64bit"]["url"].endswith(f"/bareline-{core}-windows-x64-portable.zip"))

    def test_rejects_tags_versions_and_hashes_that_do_not_match_the_release(self):
        for tag in ("v0.2.0", "v0.2.0-beta.1", "0.2.0-preview.1", "v0.2.0-preview.1x"):
            with self.assertRaisesRegex(scoop.ManifestError, "checkver regex"):
                scoop.render(scoop.TEMPLATE, self.artifacts, tag, "0.2.0")
        with self.assertRaisesRegex(scoop.ManifestError, "Cargo version"):
            scoop.render(scoop.TEMPLATE, self.artifacts, "v0.3.0-preview.1", "0.2.0")
        self.write_sums({ZIP: "f" * 64})
        with self.assertRaisesRegex(scoop.ManifestError, "actual hash"):
            scoop.render(scoop.TEMPLATE, self.artifacts, TAG, "0.2.0")
        (self.artifacts / ZIP).unlink()
        with self.assertRaisesRegex(scoop.ManifestError, "portable ZIP"):
            scoop.render(scoop.TEMPLATE, self.artifacts, TAG, "0.2.0")

    def test_rejects_templates_scoop_could_not_update_or_that_keep_portable_mode(self):
        cases = {
            "differs from its autoupdate": lambda m: m.update(version="0.2.0-preview.1"),
            "portable marker": lambda m: m.pop("post_install"),
            "prereleases": lambda m: m.update(checkver="github"),
            "unsupported Scoop substitution": lambda m: m["autoupdate"]["architecture"]["64bit"].update(
                url=m["autoupdate"]["architecture"]["64bit"]["url"].replace("$matchCore", "$matchMissing")),
            "lowercase SHA-256": lambda m: m["architecture"]["64bit"].update(hash="ABC"),
        }
        for message, change in cases.items():
            with self.subTest(message), self.assertRaisesRegex(scoop.ManifestError, message):
                scoop.render(self.template(change), self.artifacts, TAG, "0.2.0")

    def test_command_writes_a_new_manifest_only(self):
        output = self.root / "scoop" / "bareline.json"
        arguments = ["--artifact-dir", str(self.artifacts), "--tag", TAG, "--cargo-version", "0.2.0", "--output", str(output)]
        with mock.patch("sys.argv", ["scoop_manifest.py", *arguments]):
            self.assertEqual(scoop.main(), 0)
            written = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(written["architecture"]["64bit"]["hash"], self.digest)
            before = copy.deepcopy(written)
            self.assertEqual(scoop.main(), 1)
        self.assertEqual(json.loads(output.read_text(encoding="utf-8")), before)


if __name__ == "__main__":
    unittest.main()
