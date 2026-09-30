# SPDX-License-Identifier: MPL-2.0
"""Synthetic pipeline oracles; no release build, signing, network or product run."""
import base64
import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import time
import unittest
import zipfile

import release_config as config_api
import release_pipeline as pipeline

# The product's own trust code (crates/distribution), built by the CI tooling job.
VERIFIER = config_api.ROOT/'target/debug/examples/verify-authority.exe'
OPENSSL = shutil.which('openssl') or r'C:\Program Files\Git\usr\bin\openssl.exe'

def pe(extra=b'', x86=False):
    data = bytearray(512)
    data[:2] = b'MZ'; struct.pack_into('<I', data, 60, 64)
    data[64:68] = b'PE\0\0'; struct.pack_into('<H', data, 68, 0x8664)
    struct.pack_into('<H', data, 84, 240); struct.pack_into('<H', data, 88, 0x20b)
    struct.pack_into('<I', data, 196, 16)
    if x86:
        struct.pack_into('<H', data, 68, 0x14c)
        struct.pack_into('<H', data, 88, 0x10b)
        struct.pack_into('<I', data, 180, 16)
    return bytes(data)+extra

def signed_pe(data):
    # An intentionally invalid certificate blob: this exercises byte boundaries
    # only. Windows publisher verification must still reject it for packaging.
    result = bytearray(data)
    checksum, security = pipeline.pe_offsets(data, allow_x86=True)
    start = (len(data)+7)//8*8
    struct.pack_into('<I', result, checksum, 123)
    struct.pack_into('<II', result, security, start, 16)
    result.extend(b'\0'*(start-len(data))+b'NOT A SIGNATURE!')
    result.extend(b'!'*(start+16-len(result)))
    return bytes(result)

class PipelineTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='bareline-pipeline-test-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.config = copy.deepcopy(config_api.load_json(config_api.ROOT/'release/fixtures/nonshipping.release-config.json'))
        self.config['mode'] = 'configured'
        self.config['updates']['host'] = 'releases.example.org'
        for field, number in [('release_public_key',31),('catalog_public_key',32),('offline_root_public_key',33)]:
            self.config['trust'][field] = base64.b64encode(b'EdTESTONLY'+bytes([number])*32).decode()
        self.config['trust']['authenticode_subject'] = 'Synthetic Release Signer'
        self.config['trust']['authenticode_issuers'] = ['Synthetic Code Signing CA 2021', 'Synthetic Code Signing CA 2025']
        self.config_path = self.root/'config.json'
        pipeline.write_json(self.config_path, self.config)
        self.first = self.replica('first')
        self.second = self.replica('second')

    def replica(self, name):
        root = self.root/name
        (root/'unsigned-executables').mkdir(parents=True)
        (root/'components').mkdir()
        manifest = {'schema_version':1, 'configuration_version':self.config['configuration_version'],
            'build_mode':'configured', 'configuration_sha256':hashlib.sha256(config_api.canonical_bytes(self.config)).hexdigest(),
            'source_revision':'a'*40, 'source_sha256':'b'*64, 'source_identity_algorithm':config_api.SOURCE_IDENTITY_ALGORITHM,
            'source_working_tree_dirty':False, 'distribution_version':'0.1.0', 'channel':'stable',
            'features':'updates=configured,extensions=configured,runtime=external', 'runtime_packaging':'separate', 'artifacts':{}}
        for role, file, component in [('editor','bareline.exe','editor'),('helper','bareline-update-helper.exe','update-helper'),('runtime','bareline-extension-host.exe','extension-runtime')]:
            assertion = (f'BARELINE-CAPABILITY|component={component}|mode=configured|config-version={manifest["configuration_version"]}'
                f'|config={manifest["configuration_sha256"]}|source={manifest["source_sha256"]}|version=0.1.0|features={manifest["features"]}').encode()
            path = root/'unsigned-executables'/file
            path.write_bytes(pe(assertion))
            manifest['artifacts'][role] = {'file':file, **pipeline.record(path)}
        pipeline.write_json(root/'build-capabilities.json', manifest)
        inventory = {key: manifest[key] for key in ('configuration_sha256','source_sha256','source_revision','build_mode','features','distribution_version')}
        entries = []
        for component in pipeline.COMPONENTS:
            path = root/'components'/f'{component}.blex'
            with zipfile.ZipFile(path, 'w') as archive:
                for file, content in [('manifest.toml', (config_api.ROOT/f'extensions/{component}/manifest.toml').read_bytes()),('extension.wasm', b'\0asm\x01\0\0\0')]:
                    archive.writestr(zipfile.ZipInfo(file, (2020,1,1,0,0,0)), content)
            entries.append({'role':component, 'file':path.name, **pipeline.record(path)})
        pipeline.write_json(root/'components/components-inventory.json', inventory | {'inventory':'components','signed':False,'artifacts':entries})
        pipeline.write_json(root/'build-environment.json', {'schema_version':1,'kind':'configured_build_environment','clean_target':True,
            'components':True,'run_id':name,'host':'synthetic fixture','os':'test','compiler':'not a real compiler receipt',
            'target':name,'started_utc':'2026-09-16T00:00:00Z','completed_utc':'2026-09-16T00:01:00Z'})
        return root

    def compare(self):
        return pipeline.compare(self.config_path, self.first, self.second, self.root/'handoff with spaces')

    def test_relocation_and_retained_identity_after_build_caches_removed(self):
        handoff = self.compare()
        shutil.rmtree(self.first); shutil.rmtree(self.second)
        relocated = self.root/'relocated'; handoff.rename(relocated)
        _, result = pipeline.verify_handoff(relocated)
        self.assertFalse(result['release_approved'])
        self.assertEqual(result['independent_host_review'], 'pending')
        (relocated/'components/json-tools.blex').write_bytes(b'tampered')
        with self.assertRaisesRegex(ValueError, 'handoff changed'):
            pipeline.verify_handoff(relocated)

    def test_same_run_or_changed_replica_is_rejected(self):
        with self.assertRaisesRegex(ValueError, 'distinct build'):
            pipeline.compare(self.config_path, self.first, self.first, self.root/'same')
        path = self.second/'unsigned-executables/bareline.exe'
        path.write_bytes(path.read_bytes()+b'changed')
        with self.assertRaises(config_api.ConfigurationError):
            self.compare()

    def test_fixture_build_and_wrong_component_provenance_cannot_reach_signing(self):
        path = self.first/'build-capabilities.json'
        manifest = pipeline.read_json(path); manifest['build_mode'] = 'fixture'
        path.write_text(json.dumps(manifest))
        with self.assertRaisesRegex(ValueError, 'preview/fixture'):
            self.compare()
        manifest['build_mode'] = 'configured'; path.write_text(json.dumps(manifest))
        path = self.first/'components/components-inventory.json'
        inventory = pipeline.read_json(path); inventory['source_sha256'] = 'c'*64
        path.write_text(json.dumps(inventory))
        with self.assertRaisesRegex(ValueError, 'component provenance'):
            self.compare()

    def test_signing_may_only_change_certificate_fields_and_terminal_padding(self):
        source = self.first/'unsigned-executables/bareline.exe'
        signed = self.root/'signed.exe'; signed.write_bytes(signed_pe(source.read_bytes()))
        self.assertEqual(pipeline.verify_signed_bytes(source, signed), pipeline.record(signed))
        changed = bytearray(signed.read_bytes()); changed[400] ^= 1; signed.write_bytes(changed)
        with self.assertRaisesRegex(ValueError, 'code differs'):
            pipeline.verify_signed_bytes(source, signed)
        signed.write_bytes(signed_pe(source.read_bytes())+b'unlisted overlay')
        with self.assertRaisesRegex(ValueError, 'append bounds'):
            pipeline.verify_signed_bytes(source, signed)

    def test_metadata_uses_actual_signed_hashes_and_content_addressed_components(self):
        handoff = self.compare(); signed = self.root/'signed'; signed.mkdir()
        for name in pipeline.EXES:
            (signed/name).write_bytes(signed_pe((handoff/'unsigned'/name).read_bytes()))
        output = pipeline.metadata(handoff, signed, 4102444800, self.root/'metadata', authority_expiry=4102444800)
        runtime = pipeline.read_json(output/'runtime.json')
        self.assertEqual(runtime['sha256'], pipeline.record(signed/'bareline-extension-host.exe')['sha256'])
        catalog = pipeline.read_json(output/'catalog.json')
        self.assertEqual(len(catalog['entries']), 3)
        for entry in catalog['entries']:
            self.assertEqual(pipeline.record(output/(entry['sha256']+'.blex')), {'sha256':entry['sha256'],'bytes':entry['length']})
        request = pipeline.read_json(output/'signing-request.json')
        self.assertFalse(request['release_approved'])
        self.assertEqual(request['publisher_verification'], 'required_before_packaging')

    def signed_delivery(self, expiry=4102444800, authority_expiry=None, root_transitions=None):
        handoff = self.compare(); signed = self.root/'signed'; signed.mkdir()
        for name in pipeline.EXES:
            (signed/name).write_bytes(signed_pe((handoff/'unsigned'/name).read_bytes()))
        if authority_expiry is None:
            authority_expiry = max(expiry, int(time.time())+2*pipeline.AUTHORITY_MINIMUM_LIFETIME)
        return signed, pipeline.metadata(handoff, signed, expiry, self.root/'delivery',
                                         authority_expiry=authority_expiry, root_transitions=root_transitions)

    def test_authority_expiry_is_separate_and_the_core_update_delivers_it(self):
        # SEC-02: the authority outlives the metadata, and the signed core manifest binds
        # the exact authority (and optional root chain) that the update installs.
        now = int(time.time())
        chain = self.root/'transitions.json'; chain.write_bytes(b'[]')
        _, output = self.signed_delivery(now+3600, now+3*pipeline.AUTHORITY_MINIMUM_LIFETIME, chain)
        authority = pipeline.read_json(output/'bareline.release-authority.json')
        update = pipeline.read_json(output/'bareline.update.json')
        self.assertEqual(update['expires_unix'], now+3600)
        self.assertEqual(authority['expires_unix'], now+3*pipeline.AUTHORITY_MINIMUM_LIFETIME)
        self.assertEqual(update['authority_sha256'], pipeline.record(output/'bareline.release-authority.json')['sha256'])
        self.assertEqual(update['root_transitions_sha256'], pipeline.record(chain)['sha256'])
        self.assertEqual((output/'bareline.root-transitions.json').read_bytes(), b'[]')
        # The runtime manifest delivers nothing; runtime and catalog keep their own floors.
        self.assertNotIn('authority_sha256', pipeline.read_json(output/'runtime.json'))
        request = pipeline.read_json(output/'signing-request.json')
        self.assertIn('bareline.root-transitions.json', request['update_host_next_to_manifest'])
        self.assertIn('bareline-update-helper.exe', request['update_host_next_to_manifest'])

    def test_authority_expiry_cannot_follow_the_metadata_expiry(self):
        now = int(time.time())
        handoff = self.compare(); signed = self.root/'signed'; signed.mkdir()
        for name in pipeline.EXES:
            (signed/name).write_bytes(signed_pe((handoff/'unsigned'/name).read_bytes()))
        for metadata_expiry, authority_expiry in [(now+3600, now+3600), (now+3600, now+30*24*3600),
                                                  (now+4*pipeline.AUTHORITY_MINIMUM_LIFETIME,
                                                   now+2*pipeline.AUTHORITY_MINIMUM_LIFETIME)]:
            with self.assertRaisesRegex(ValueError, 'authority expiry'):
                pipeline.metadata(handoff, signed, metadata_expiry, self.root/'refused', authority_expiry=authority_expiry)
            self.assertFalse((self.root/'refused').exists())
        output = pipeline.metadata(handoff, signed, now+3600, self.root/'separate',
                                   authority_expiry=now+2*pipeline.AUTHORITY_MINIMUM_LIFETIME)
        self.assertNotIn('root_transitions_sha256', pipeline.read_json(output/'bareline.update.json'))
        self.assertNotIn('bareline.root-transitions.json',
                         pipeline.read_json(output/'signing-request.json')['update_host_next_to_manifest'])

    def test_metadata_carries_one_publisher_identity_and_a_separate_signer_pin(self):
        signed, output = self.signed_delivery()
        publisher = self.config['trust']['publisher']
        for name in ('bareline.update.json', 'runtime.json'):
            self.assertEqual(pipeline.read_json(output/name)['publisher'], publisher)
        for entry in pipeline.read_json(output/'catalog.json')['entries']:
            self.assertEqual(entry['publisher'], publisher)
        authority = pipeline.read_json(output/'bareline.release-authority.json')
        self.assertNotIn('publisher_certificate_sha256', authority)
        self.assertEqual(authority['authenticode_subject'], self.config['trust']['authenticode_subject'])
        self.assertEqual(authority['authenticode_issuers'], self.config['trust']['authenticode_issuers'])
        self.assertEqual(authority['update_helper_sha256'], pipeline.record(signed/'bareline-update-helper.exe')['sha256'])

    @unittest.skipUnless(Path(OPENSSL).exists(), 'OpenSSL Ed25519 signing needed for the product trust round trip')
    def test_pipeline_output_verifies_with_the_product_trust_code(self):
        # SEC-01: sign the pipeline's own output with ephemeral keys and verify it with the
        # Rust policy the app and update helper use (core_update_policy + verify_manifest).
        keys = {}
        for role in ('root', 'release', 'catalog'):
            private = self.root/f'{role}.der'
            private.write_bytes(bytes.fromhex('302e020100300506032b657004220420')+os.urandom(32))
            public = subprocess.check_output([OPENSSL,'pkey','-inform','DER','-in',str(private),'-pubout','-outform','DER'])[-32:]
            keys[role] = (private, base64.b64encode(b'EdTESTONLY'+public).decode())
        def sign(path, role):
            def ed(message):
                source = self.root/'message'; source.write_bytes(message)
                output = self.root/'signature'
                subprocess.run([OPENSSL,'pkeyutl','-sign','-rawin','-inkey',str(keys[role][0]),'-keyform','DER',
                                '-in',str(source),'-out',str(output)], check=True, capture_output=True)
                return output.read_bytes()
            signature = ed(hashlib.blake2b(path.read_bytes()).digest()); comment = b'EPHEMERAL NONSHIPPING TEST'
            return ('untrusted comment: ephemeral test\n'+base64.b64encode(b'EDTESTONLY'+signature).decode()
                    +'\ntrusted comment: '+comment.decode()+'\n'+base64.b64encode(ed(signature+comment)).decode()+'\n')
        signatures = {'bareline.update.json': ('bareline.update.minisig', 'release'), 'runtime.json': ('runtime.minisig', 'release'),
                      'catalog.json': ('catalog.json.minisig', 'catalog'),
                      'bareline.release-authority.json': ('bareline.release-authority.minisig', 'root')}
        self.config['trust'].update(release_public_key=keys['release'][1], catalog_public_key=keys['catalog'][1],
                                    offline_root_public_key=keys['root'][1])
        self.config_path = self.root/'product-config.json'
        pipeline.write_json(self.config_path, self.config)
        self.first, self.second = self.replica('product-first'), self.replica('product-second')
        _, delivery = self.signed_delivery(int(time.time())+3600)
        request = pipeline.read_json(delivery/'signing-request.json')
        self.assertEqual(set(request['signatures']), set(signatures))
        for name, (signature, role) in signatures.items():
            (delivery/signature).write_text(sign(delivery/name, role), encoding='ascii')
        if not VERIFIER.exists():
            self.skipTest('build the verifier first: cargo build --locked -p bareline-distribution --example verify-authority')
        def verify():
            return subprocess.run([str(VERIFIER), '--config', str(self.config_path), '--directory', str(delivery),
                '--delivery-directory', str(delivery), '--now', str(int(time.time()))], capture_output=True, text=True, timeout=60)
        result = verify()
        self.assertEqual(result.returncode, 0, result.stderr)
        authority = json.loads(result.stdout)['authority']
        self.assertEqual(authority['authenticode_issuers'], self.config['trust']['authenticode_issuers'])
        # A manifest whose publisher is not trust.publisher (e.g. a certificate digest) is refused.
        update = delivery/'bareline.update.json'; original = update.read_bytes()
        manifest = json.loads(original); manifest['publisher'] = '07'*32
        update.write_text(json.dumps(manifest)); (delivery/'bareline.update.minisig').write_text(sign(update, 'release'), encoding='ascii')
        self.assertNotEqual(verify().returncode, 0)
        update.write_bytes(original); (delivery/'bareline.update.minisig').write_text(sign(update, 'release'), encoding='ascii')
        self.assertEqual(verify().returncode, 0)
        # The helper the app launches must be the one the signed authority pins.
        helper = delivery/'bareline-update-helper.exe'
        helper.write_bytes(helper.read_bytes()+b'changed')
        self.assertNotEqual(verify().returncode, 0)

    def test_handoff_refuses_reuse_and_corrupt_headers(self):
        self.compare()
        with self.assertRaisesRegex(ValueError, 'new output'):
            self.compare()
        for malformed in (b'', b'MZ'+b'\0'*62, pe()[:150]):
            with self.assertRaises(ValueError):
                pipeline.pe_offsets(malformed)

    def final_inputs(self):
        handoff = self.compare()
        assembled = self.root/'assembled with spaces'; assembled.mkdir()
        installer = assembled/'bareline-0.1.0-windows-x64-setup.exe'
        installer.write_bytes(pe(b'INNO SETUP PAYLOAD', x86=True))
        signed_installer = self.root/'external signed installer.exe'
        signed_installer.write_bytes(signed_pe(installer.read_bytes()))
        with zipfile.ZipFile(assembled/'bareline-0.1.0-windows-x64-portable.zip', 'w') as archive:
            for name in pipeline.EXES[:2]:
                archive.writestr(name, signed_pe((handoff/'unsigned'/name).read_bytes()))
        (assembled/'bareline-exthost-x64.exe').write_bytes(signed_pe((handoff/'unsigned'/pipeline.EXES[2]).read_bytes()))
        (assembled/'SHA-256SUMS').write_text('obsolete pre-signing hashes')
        return handoff, assembled, signed_installer

    def test_final_inventory_uses_external_signed_installer_and_retains_inputs(self):
        handoff, assembled, signed_installer = self.final_inputs()
        output = pipeline.prepare_final(handoff, assembled, signed_installer, self.root/'final request')
        request = pipeline.verify_final_request(output, handoff)
        self.assertFalse(request['release_approved'])
        self.assertEqual(request['signature']['key_role'], 'release_public_key')
        installer_name = 'bareline-0.1.0-windows-x64-setup.exe'
        self.assertEqual(request['files'][installer_name], pipeline.record(signed_installer))
        sums = (output/'artifacts/SHA-256SUMS').read_text()
        self.assertIn(pipeline.record(signed_installer)['sha256']+'  '+installer_name, sums)
        self.assertEqual((assembled/'SHA-256SUMS').read_text(), 'obsolete pre-signing hashes')
        with self.assertRaisesRegex(ValueError, 'new output'):
            pipeline.prepare_final(handoff, assembled, signed_installer, output)

    def test_finalization_detects_changed_inputs_before_inventory_signature_import(self):
        handoff, assembled, installer = self.final_inputs()
        output = pipeline.prepare_final(handoff, assembled, installer, self.root/'prepared')
        (output/'artifacts/unexpected.txt').write_text('unlisted')
        with self.assertRaisesRegex(ValueError, 'file set changed'):
            pipeline.verify_final_request(output, handoff)
        (output/'artifacts/unexpected.txt').unlink()
        sums = output/'artifacts/SHA-256SUMS'; original = sums.read_bytes()
        sums.write_bytes(original+b'changed')
        with self.assertRaisesRegex(ValueError, 'file changed'):
            pipeline.verify_final_request(output, handoff)
        sums.write_bytes(original)
        (output/'public-release-config.json').write_text('{}')
        with self.assertRaisesRegex(ValueError, 'config changed'):
            pipeline.verify_final_request(output, handoff)

    def test_prepared_request_is_not_a_replacement_trust_root(self):
        handoff, assembled, installer = self.final_inputs()
        output = pipeline.prepare_final(handoff, assembled, installer, self.root/'prepared')
        request_path = output/'finalization-request.json'
        request = pipeline.read_json(request_path)
        config_path = output/'public-release-config.json'
        config = pipeline.read_json(config_path)
        config['updates']['host'] = 'replacement.example.org'
        config_path.write_text(json.dumps(config))
        request['configuration'] = pipeline.record(config_path)
        request_path.write_text(json.dumps(request))
        with self.assertRaisesRegex(ValueError, 'config changed'):
            pipeline.verify_final_request(output, handoff)
        shutil.copyfile(handoff/'public-release-config.json', config_path)
        request['configuration'] = pipeline.record(config_path)
        request['unsigned_handoff_sha256'] = '1'*64
        request_path.write_text(json.dumps(request))
        with self.assertRaisesRegex(ValueError, 'handoff identity changed'):
            pipeline.verify_final_request(output, handoff)
        request['unsigned_handoff_sha256'] = pipeline.record(handoff/'handoff.json')['sha256']
        runtime = output/'artifacts/bareline-exthost-x64.exe'
        data = bytearray(runtime.read_bytes()); data[400] ^= 1; runtime.write_bytes(data)
        request['files'][runtime.name] = pipeline.record(runtime)
        request_path.write_text(json.dumps(request))
        with self.assertRaisesRegex(ValueError, 'code differs'):
            pipeline.verify_final_request(output, handoff)

    def test_only_matching_stable_tag_can_request_configured_signing(self):
        pipeline.verify_release_tag(self.config_path, 'v0.1.0')
        for tag in ('v0.1.0-preview.1', 'v0.2.0', '0.1.0', 'V0.1.0', 'refs/tags/v0.1.0'):
            with self.subTest(tag=tag), self.assertRaisesRegex(ValueError, 'configured release tag'):
                pipeline.verify_release_tag(self.config_path, tag)

    @unittest.skipUnless(shutil.which('pwsh'), 'PowerShell 7 needed for finalization wrapper test')
    def test_finalization_wrapper_stops_before_signature_import_on_tampered_request(self):
        handoff, assembled, installer = self.final_inputs()
        prepared = pipeline.prepare_final(handoff, assembled, installer, self.root/'prepared')
        (prepared/'artifacts/SHA-256SUMS').write_text('tampered inventory')
        output = self.root/'must not be created'
        result = subprocess.run([
            shutil.which('pwsh'), '-NoProfile', '-File',
            str(config_api.ROOT/'packaging/windows/finalize-configured.ps1'),
            '-Handoff', str(handoff), '-Prepared', str(prepared),
            '-InventorySignature', str(self.root/'not imported.minisig'),
            '-Minisign', 'must-not-run.exe', '-AuthorityVerifier', 'must-not-run.exe',
            '-OutputDir', str(output),
        ], capture_output=True, text=True, timeout=30)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Prepared final inventory changed', result.stderr)
        self.assertFalse(output.exists())

    def test_finalization_rejects_replaced_installer_or_portable_helper_code(self):
        handoff, assembled, installer = self.final_inputs()
        original = installer.read_bytes()
        changed = bytearray(original); changed[400] ^= 1; installer.write_bytes(changed)
        with self.assertRaisesRegex(ValueError, 'code differs'):
            pipeline.prepare_final(handoff, assembled, installer, self.root/'bad-installer')
        self.assertFalse((self.root/'bad-installer').exists())
        installer.write_bytes(original)
        with zipfile.ZipFile(assembled/'bareline-0.1.0-windows-x64-portable.zip', 'w') as archive:
            for name in pipeline.EXES[:2]:
                data = bytearray((handoff/'unsigned'/name).read_bytes())
                if name == 'bareline-update-helper.exe':
                    data[400] ^= 1
                archive.writestr(name, signed_pe(data))
        with self.assertRaisesRegex(ValueError, 'code differs'):
            pipeline.prepare_final(handoff, assembled, installer, self.root/'bad-helper')
        self.assertFalse((self.root/'bad-helper').exists())

    def test_pe32_is_allowed_for_installer_only_and_metadata_expiry_must_be_future(self):
        source = self.root/'setup.exe'; source.write_bytes(pe(x86=True))
        signed = self.root/'signed-setup.exe'; signed.write_bytes(signed_pe(source.read_bytes()))
        with self.assertRaisesRegex(ValueError, 'x64'):
            pipeline.verify_signed_bytes(source, signed)
        pipeline.verify_signed_bytes(source, signed, allow_x86=True)
        with self.assertRaisesRegex(ValueError, 'future expiry'):
            pipeline.metadata(self.compare(), self.root/'unused', 1, self.root/'expired', authority_expiry=4102444800)

if __name__ == '__main__':
    unittest.main()
