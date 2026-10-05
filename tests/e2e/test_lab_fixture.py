# SPDX-License-Identifier: MPL-2.0
"""Headless safety/observer tests; synthetic assets are never executed."""
import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import lab_fixture as lab
import native_adapter

class LabTests(unittest.TestCase):
    def fixture(self,root):
        source=root/'probe.exe';source.write_bytes(b'synthetic parser input, never execute')
        return dict(schema_version=1,journey='crash_recovery',machine_uuid='12345678-1234-1234-1234-123456789abc',snapshot_id='synthetic-only',
                    save_point='StageFlushed',assets=[dict(id='recovery_probe',path=str(source),relative='probe.exe',sha256=lab.sha(source))])

    def test_explicit_inputs_copy_once_with_hash_and_preserve_sources(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);config=root/'lab.json';doc=self.fixture(root);config.write_text(json.dumps(doc));scratch=root/'scratch';scratch.mkdir()
            prepared=lab.prepare(config,'crash_recovery',scratch)
            self.assertEqual(prepared['identity']['lab_config_sha256'],lab.sha(config))
            self.assertEqual(lab.sha(Path(prepared['paths']['recovery_probe'])),doc['assets'][0]['sha256'])
            with self.assertRaises(FileExistsError):lab.prepare(config,'crash_recovery',scratch)
            self.assertEqual(lab.sha(root/'probe.exe'),doc['assets'][0]['sha256'])

    def test_wrong_journey_missing_probe_tampered_asset_and_path_escape_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);config=root/'lab.json';original=self.fixture(root)
            mutations=[lambda d:d.update(journey='extension_isolation'),lambda d:d.update(assets=[]),
                       lambda d:d['assets'][0].update(sha256='0'*64),lambda d:d['assets'][0].update(relative='../outside.exe'),
                       lambda d:d['assets'].append(copy.deepcopy(d['assets'][0])),lambda d:d.update(save_point='sleep-then-kill'),lambda d:d.update(schema_version=True)]
            for mutate in mutations:
                doc=copy.deepcopy(original);mutate(doc);config.write_text(json.dumps(doc))
                with self.assertRaises(ValueError):lab.load(config,'crash_recovery')

    def test_every_lab_journey_requires_explicit_inputs_before_launch(self):
        for journey in lab.JOURNEYS:
            request=dict(journey=dict(id=journey),mode='keyboard',theme='dark',dpi='100')
            self.assertIn('--lab-config',native_adapter.unavailable(request))
            self.assertIsNone(native_adapter.unavailable(request,Path('explicit-input')))

    def test_stage_labels_alone_cannot_forge_crash_pass(self):
        steps=[dict(id='s'+str(i),status='PASS') for i in range(1,4)]
        records=[dict(stage=name,details=dict(fake=True)) for group in lab.RECORDS['crash_recovery'] for name in group]
        with self.assertRaisesRegex(ValueError,'Durable copies'):lab.validate_observations('crash_recovery',steps,records)
        steps[0]['status']='NOT_RUN';steps[2]['status']='NOT_RUN'
        by_name={r['stage']:r for r in records}
        by_name['observed save boundary']['details']=dict(pid=8,point='StageFlushed',token='a'*32)
        by_name['owned editor terminated']['details']=dict(pid=9,boundary='StageFlushed')
        with self.assertRaisesRegex(ValueError,'Foreign'):lab.validate_observations('crash_recovery',steps,records)
        by_name['owned editor terminated']['details']['pid']=8
        by_name['atomic saved bytes']['details']=dict(sha256=hashlib.sha256(b'partial').hexdigest())
        with self.assertRaisesRegex(ValueError,'Partial'):lab.validate_observations('crash_recovery',steps,records)

    def test_not_run_never_needs_or_infers_observations(self):
        for journey in lab.JOURNEYS:
            steps=[dict(id='s'+str(i),status='NOT_RUN') for i in range(1,4)]
            lab.validate_observations(journey,steps,[])

if __name__=='__main__':unittest.main()
