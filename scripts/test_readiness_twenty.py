# SPDX-License-Identifier: MPL-2.0
"""Focused release-configuration checks for the twenty-task readiness batch."""
import base64
import copy
import json
import os
import subprocess
from pathlib import Path
import tempfile
import unittest

import release_config as config


class OfflineRootTests(unittest.TestCase):
    def fixture(self):
        return config.load_json(config.ROOT / 'release/fixtures/nonshipping.release-config.json')

    def test_03_prepared_policy_and_config_digest_cannot_be_rewritten(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / 'fixture.json'
            fixture = self.fixture()
            source.write_text(json.dumps(fixture), encoding='utf-8')
            prepared = root / 'prepared.txt'
            config.prepare(source, prepared, 'fixture')
            original = prepared.read_bytes()
            values = config.verify_prepared(prepared, 'fixture')
            self.assertEqual(values['BARELINE_OFFLINE_ROOT_PUBLIC_KEY'], fixture['trust']['offline_root_public_key'])
            self.assertEqual(values['BARELINE_ROOT_VERSION_FLOOR'], '1')
            for key, wrong in (('BARELINE_OFFLINE_ROOT_PUBLIC_KEY', fixture['trust']['release_public_key']),
                               ('BARELINE_ROOT_VERSION_FLOOR', '2')):
                changed = dict(values, **{key: wrong})
                prepared.write_bytes(config.serialize_prepared(changed))
                with self.subTest(key=key), self.assertRaisesRegex(config.ConfigurationError, 'derivation'):
                    config.verify_prepared(prepared, 'fixture')
            prepared.write_bytes(original)
            fixture['trust']['minimum_root_version'] = 2
            derived = config.derive_values(fixture, config.source_identity())
            self.assertNotEqual(derived['BARELINE_CONFIG_DIGEST'], values['BARELINE_CONFIG_DIGEST'])

    def test_04_build_handoff_overrides_ambient_policy_and_rejects_tampering(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            harness = root / 'handoff.rs'
            module = (config.ROOT / 'build-support/release_config.rs').as_posix()
            harness.write_text(f'#[path = "{module}"] mod release; fn main() {{ release::configure("probe"); }}', encoding='utf-8')
            executable = root / ('handoff.exe' if os.name == 'nt' else 'handoff')
            subprocess.run(['rustc', '--edition=2024', str(harness), '-o', str(executable)], check=True, capture_output=True)
            environment = dict(os.environ)
            for key in ('CARGO_FEATURE_CONFIGURED_RELEASE', 'CARGO_FEATURE_FIXTURE_RELEASE', 'BARELINE_PREPARED_RELEASE_CONFIG'):
                environment.pop(key, None)
            environment.update(CARGO_MANIFEST_DIR=str(config.ROOT / 'apps/bareline'), CARGO_PKG_VERSION=config.workspace_version(),
                               BARELINE_OFFLINE_ROOT_PUBLIC_KEY='uncontrolled', BARELINE_ROOT_VERSION_FLOOR='999')
            def invoke():
                return subprocess.run([str(executable)], env=environment, cwd=config.ROOT, capture_output=True, text=True)
            preview = invoke()
            self.assertEqual(preview.returncode, 0, preview.stderr)
            self.assertIn('cargo:rustc-env=BARELINE_OFFLINE_ROOT_PUBLIC_KEY=disabled', preview.stdout)
            self.assertIn('cargo:rustc-env=BARELINE_ROOT_VERSION_FLOOR=disabled', preview.stdout)
            prepared = root / 'prepared.txt'
            config.prepare(config.ROOT / 'release/fixtures/nonshipping.release-config.json', prepared, 'fixture')
            environment.update(CARGO_FEATURE_FIXTURE_RELEASE='1', BARELINE_PREPARED_RELEASE_CONFIG=str(prepared))
            fixture = invoke()
            self.assertEqual(fixture.returncode, 0, fixture.stderr)
            self.assertIn('cargo:rustc-env=BARELINE_OFFLINE_ROOT_PUBLIC_KEY=' + self.fixture()['trust']['offline_root_public_key'], fixture.stdout)
            self.assertIn('cargo:rustc-env=BARELINE_ROOT_VERSION_FLOOR=1', fixture.stdout)
            prepared.write_bytes(prepared.read_bytes().replace(b'BARELINE_ROOT_VERSION_FLOOR=1', b'BARELINE_ROOT_VERSION_FLOOR=999'))
            invalid = invoke()
            self.assertNotEqual(invalid.returncode, 0)
            self.assertNotIn('cargo:rustc-env=BARELINE_OFFLINE_ROOT_PUBLIC_KEY=', invalid.stdout)

    def test_01_root_policy_is_required_and_floor_is_bounded(self):
        fixture = self.fixture()
        config.validate_document(fixture)
        for field in ('offline_root_public_key', 'minimum_root_version'):
            changed = copy.deepcopy(fixture)
            del changed['trust'][field]
            with self.subTest(missing=field), self.assertRaises(config.ConfigurationError):
                config.validate_document(changed)
        for floor in (0, -1, True, 1.0, 2**64):
            changed = copy.deepcopy(fixture)
            changed['trust']['minimum_root_version'] = floor
            with self.subTest(floor=floor), self.assertRaises(config.ConfigurationError):
                config.validate_document(changed)
        preview = config.load_json(config.ROOT / 'release/preview.release-config.json')
        config.validate_document(preview)
        preview['trust']['minimum_root_version'] = 1
        with self.assertRaises(config.ConfigurationError):
            config.validate_document(preview)

    def test_02_root_keys_are_independent_and_nonshipping_pins_cannot_be_aliased(self):
        fixture = self.fixture()
        for key in ('release_public_key', 'catalog_public_key'):
            changed = copy.deepcopy(fixture)
            material = base64.b64decode(changed['trust'][key])[10:]
            changed['trust']['offline_root_public_key'] = base64.b64encode(b'EdOTHERKEY' + material).decode()
            with self.subTest(key=key), self.assertRaisesRegex(config.ConfigurationError, 'independently'):
                config.validate_document(changed)
        for invalid in ('', 'disabled', 'RW' + 'x'*54):
            changed = copy.deepcopy(fixture)
            changed['trust']['offline_root_public_key'] = invalid
            with self.subTest(invalid=invalid), self.assertRaises(config.ConfigurationError):
                config.validate_document(changed)
        configured = copy.deepcopy(fixture)
        configured['mode'] = 'configured'
        configured['updates']['host'] = 'releases.bareline.app'
        configured['trust']['publisher_certificate_sha256'] = '1'*64
        for key, byte in (('release_public_key', 21), ('catalog_public_key', 22), ('offline_root_public_key', 23)):
            configured['trust'][key] = base64.b64encode(b'EdTESTONLY' + bytes([byte])*32).decode()
        config.validate_document(configured)
        material = base64.b64decode(fixture['trust']['offline_root_public_key'])[10:]
        configured['trust']['offline_root_public_key'] = base64.b64encode(b'EdOTHERKEY' + material).decode()
        with self.assertRaisesRegex(config.ConfigurationError, 'nonshipping'):
            config.validate_document(configured)


if __name__ == '__main__':
    unittest.main()
