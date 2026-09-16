# SPDX-License-Identifier: MPL-2.0
"""Synthetic pipeline oracles; no release build, signing, network or product run."""
import base64
import copy
import hashlib
import json
from pathlib import Path
import shutil
import struct
import tempfile
import unittest
import zipfile

import release_config as config_api
import release_pipeline as pipeline

def pe(extra=b''):
    data = bytearray(512)
    data[:2] = b'MZ'; struct.pack_into('<I', data, 60, 64)
    data[64:68] = b'PE\0\0'; struct.pack_into('<H', data, 68, 0x8664)
    struct.pack_into('<H', data, 84, 240); struct.pack_into('<H', data, 88, 0x20b)
    struct.pack_into('<I', data, 196, 16)
    return bytes(data)+extra

def signed_pe(data):
    # An intentionally invalid certificate blob: this exercises byte boundaries
    # only. Windows publisher verification must still reject it for packaging.
    result = bytearray(data)
    checksum, security = pipeline.pe_offsets(data)
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
        self.config['trust']['publisher_certificate_sha256'] = '11'*32
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
        output = pipeline.metadata(handoff, signed, 4102444800, self.root/'metadata')
        runtime = pipeline.read_json(output/'runtime.json')
        self.assertEqual(runtime['sha256'], pipeline.record(signed/'bareline-extension-host.exe')['sha256'])
        catalog = pipeline.read_json(output/'catalog.json')
        self.assertEqual(len(catalog['entries']), 3)
        for entry in catalog['entries']:
            self.assertEqual(pipeline.record(output/(entry['sha256']+'.blex')), {'sha256':entry['sha256'],'bytes':entry['length']})
        request = pipeline.read_json(output/'signing-request.json')
        self.assertFalse(request['release_approved'])
        self.assertEqual(request['publisher_verification'], 'required_before_packaging')

    def test_handoff_refuses_reuse_and_corrupt_headers(self):
        self.compare()
        with self.assertRaisesRegex(ValueError, 'new output'):
            self.compare()
        for malformed in (b'', b'MZ'+b'\0'*62, pe()[:150]):
            with self.assertRaises(ValueError):
                pipeline.pe_offsets(malformed)

if __name__ == '__main__':
    unittest.main()
