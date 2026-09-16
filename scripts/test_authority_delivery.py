# SPDX-License-Identifier: MPL-2.0
"""Synthetic local packaging tests. No signing, network, installer or product launch."""
import json
import base64
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
import zipfile

ROOT = Path(__file__).resolve().parents[1]
VECTORS = ROOT / 'crates/distribution/tests/fixtures/authority'
VERIFIER = ROOT / 'target/debug/examples/verify-authority.exe'
POWERSHELL = shutil.which('pwsh') or shutil.which('powershell')
OPENSSL = shutil.which('openssl') or r'C:\Program Files\Git\usr\bin\openssl.exe'

class AuthorityDelivery(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='bareline-authority-delivery-')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.payload = self.root / 'payload with spaces'
        self.payload.mkdir()
        for name in ('bareline.exe', 'bareline-update-helper.exe', 'LICENSE', 'THIRD-PARTY-NOTICES.md', 'SBOM.json'):
            (self.payload/name).write_text('SYNTHETIC NONSHIPPING '+name, encoding='ascii')
        # Ephemeral test-only keys are deleted with this test directory. Checked-in
        # deterministic vector keys must be refused by the shipping config gate.
        keys = {}
        for role in ('root','release','catalog'):
            private = self.root/f'{role}.der'
            private.write_bytes(bytes.fromhex('302e020100300506032b657004220420')+os.urandom(32))
            public = subprocess.check_output([OPENSSL,'pkey','-inform','DER','-in',str(private),'-pubout','-outform','DER'])[-32:]
            keys[role] = base64.b64encode(b'EdTESTONLY'+public).decode()
        authority = json.loads((VECTORS/'rotated.json').read_text())
        authority.update(release_public_key=keys['release'],catalog_public_key=keys['catalog'],revoked_release_keys=[])
        data = json.dumps(authority,sort_keys=True,separators=(',',':')).encode()
        def sign(message):
            source=self.root/'message';source.write_bytes(message)
            output=self.root/'signature'
            subprocess.run([OPENSSL,'pkeyutl','-sign','-rawin','-inkey',str(self.root/'root.der'),'-keyform','DER','-in',str(source),'-out',str(output)],check=True,capture_output=True)
            return output.read_bytes()
        signature=sign(hashlib.blake2b(data).digest());comment=b'EPHEMERAL NONSHIPPING TEST'
        self.signature=('untrusted comment: ephemeral test\n'+base64.b64encode(b'EDTESTONLY'+signature).decode()
            +'\ntrusted comment: '+comment.decode()+'\n'+base64.b64encode(sign(signature+comment)).decode()+'\n')
        (self.payload/'bareline.release-authority.json').write_bytes(data)
        (self.payload/'bareline.release-authority.minisig').write_text(self.signature,encoding='ascii')
        config = json.loads((ROOT/'release/fixtures/nonshipping.release-config.json').read_text(encoding='utf-8'))
        config['mode'] = 'configured'
        config['trust'].update({
            'release_public_key': keys['release'],
            'catalog_public_key': keys['catalog'],
            'offline_root_public_key': keys['root'],
            'publisher_certificate_sha256': '09'*32,
            'publisher': 'Synthetic Authority Test',
            'minimum_root_version': 2,
        })
        config['updates'].update(host='releases.example.org', metadata_version=5, minimum_metadata_version=5)
        self.config = self.root/'public synthetic config.json'
        self.config.write_text(json.dumps(config), encoding='utf-8')

    def package(self, output):
        return subprocess.run([POWERSHELL, '-NoProfile', '-File', str(ROOT/'packaging/windows/build.ps1'),
            '-PayloadDir', str(self.payload), '-Version', '0.1.0', '-OutputDir', str(output),
            '-ReleaseConfig', str(self.config), '-AuthorityVerifier', str(VERIFIER)], capture_output=True, text=True, timeout=45)

    def test_signed_bootstrap_bytes_reach_portable_package(self):
        output = self.root/'output'
        result = self.package(output)
        self.assertEqual(result.returncode, 0, result.stderr)
        with zipfile.ZipFile(output/'bareline-0.1.0-windows-x64-portable.zip') as archive:
            self.assertEqual(len(archive.namelist()), 8)
            for suffix in ('json', 'minisig'):
                name = f'bareline.release-authority.{suffix}'
                self.assertEqual(archive.read(name), (self.payload/name).read_bytes())

    def test_tamper_missing_signature_and_fixture_mode_fail_before_archive(self):
        original = (self.payload/'bareline.release-authority.json').read_bytes()
        (self.payload/'bareline.release-authority.json').write_bytes(original+b' ')
        output = self.root/'tampered'
        self.assertNotEqual(self.package(output).returncode, 0)
        self.assertFalse(list(output.glob('*.zip')))
        (self.payload/'bareline.release-authority.json').write_bytes(original)
        (self.payload/'bareline.release-authority.minisig').unlink()
        self.assertNotEqual(self.package(self.root/'missing').returncode, 0)
        (self.payload/'bareline.release-authority.minisig').write_text(self.signature,encoding='ascii')
        shutil.copyfile(ROOT/'release/fixtures/nonshipping.release-config.json', self.config)
        self.assertNotEqual(self.package(self.root/'fixture').returncode, 0)

    def test_verifier_accepts_dual_signature_chain_and_rejects_incomplete_chain(self):
        for suffix in ('json','minisig'):
            shutil.copyfile(VECTORS/f'rotated.{suffix}', self.payload/f'bareline.release-authority.{suffix}')
        config = json.loads(self.config.read_text())
        config['trust']['offline_root_public_key'] = (VECTORS/'root.txt').read_text()
        config['trust']['minimum_root_version'] = 1
        self.config.write_text(json.dumps(config))
        target = self.payload/'bareline.root-transitions.json'
        def verify():
            return subprocess.run([str(VERIFIER), '--config', str(self.config), '--directory', str(self.payload), '--now', '100'], capture_output=True, text=True, timeout=10)
        shutil.copyfile(VECTORS/'transitions.json', target)
        result = verify()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(json.loads(result.stdout)['root_lineage']), 2)
        shutil.copyfile(VECTORS/'bad-transitions.json', target)
        self.assertNotEqual(verify().returncode, 0)

    def test_all_public_rotation_vector_keys_are_nonshipping(self):
        import release_config
        for path in VECTORS.glob('*.txt'):
            config=json.loads(self.config.read_text())
            config['trust']['offline_root_public_key']=path.read_text()
            with self.assertRaisesRegex(release_config.ConfigurationError,'nonshipping'):
                release_config.validate_document(config)

if __name__ == '__main__':
    unittest.main()
