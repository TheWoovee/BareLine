# SPDX-License-Identifier: MPL-2.0
"""Synthetic regression oracles; never native product evidence."""
import base64
import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest

import native_adapter as adapter
import regex_transform_fixture as regex
import runner

PASS = [{'id': f's{i}', 'status': 'PASS', 'observed': 'synthetic only'} for i in range(1, 4)]


def records(path):
    result = []
    def add(stage, details): result.append({'stage': stage, 'details': details})
    for stage, text in [('regex opened exact text', regex.INITIAL), ('regex query input', regex.PATTERN),
                        ('regex setup preserved text', regex.INITIAL), ('regex replacement input', regex.REPLACEMENT),
                        ('regex preview preserved text', regex.INITIAL), ('regex replaced exact text', regex.REPLACED),
                        ('regex single Undo', regex.INITIAL)]:
        add(stage, {'text': text})
    for stage, text in [('regex setup preserved bytes', regex.INITIAL), ('regex preview preserved bytes', regex.INITIAL),
                        ('regex unsaved replacement preserved disk', regex.INITIAL), ('regex saved replacement bytes', regex.REPLACED),
                        ('regex Undo before save preserved disk', regex.REPLACED), ('regex restored original bytes', regex.INITIAL)]:
        raw = text.encode('utf-8')
        add(stage, {'bytes': len(raw), 'sha256': hashlib.sha256(raw).hexdigest(), 'base64': base64.b64encode(raw).decode()})
    add('regex mode', {'label': 'Regular Expression Search Mode', 'checked': True, 'state': 8})
    add('regex match count', {'name': 'Find results: 2 matches'})
    add('regex capture preview', {'status': {'name': regex.PREVIEW_STATUS}, 'rows': [
        {'name': name, 'control_type': 'ControlType.ListItem'} for name in [regex.preview_header(path), *regex.fixture()['preview_rows']]]})
    add('regex replacement count', {'name': regex.APPLIED_PREFIX + 'owned-receipt.json'})
    return result


class RegexTransformTests(unittest.TestCase):
    path = Path('C:/synthetic/fixture/input.txt')

    def test_exact_multiline_capture_oracle_and_unicode_offsets(self):
        self.assertEqual(regex.fixture()['preview_rows'], ['[\u2713] 17: item=alpha qty=12 \u2192 alpha:12',
                                                         '[\u2713] 53: item=beta qty=07 \u2192 beta:07'])
        self.assertEqual(regex.REPLACED, 'keep cafe\u0301 \U0001f389\nalpha:12\nbetween unchanged\nbeta:07\nkeep \u6587\n')
        regex.validate_observations(PASS, records(self.path), self.path)

    def test_every_required_checkpoint_must_be_unique_and_present(self):
        original = records(self.path)
        for index in range(len(original)):
            for changed in (original[:index] + original[index+1:], original + [original[index]]):
                with self.assertRaises(ValueError): regex.validate_observations(PASS, changed, self.path)

    def test_wrong_regex_mode_query_or_replacement_rejected(self):
        for stage, change in [('regex mode', {'checked': False}), ('regex mode', {'state': 0}),
                              ('regex mode', {'label': 'Extended Search Mode'}), ('regex query input', {'text': 'literal'}),
                              ('regex replacement input', {'text': '$1'})]:
            data=records(self.path);next(r for r in data if r['stage']==stage)['details'].update(change)
            with self.assertRaises(ValueError): regex.validate_observations(PASS, data, self.path)

    def test_incomplete_wrong_count_and_failed_apply_rejected(self):
        for name in ['Find results: 2+ matches', 'Find results: 1 matches', 'Find results: Invalid query']:
            data=records(self.path);next(r for r in data if r['stage']=='regex match count')['details']['name']=name
            with self.assertRaises(ValueError): regex.validate_observations(PASS, data, self.path)
        data=records(self.path)
        next(r for r in data if r['stage']=='regex replacement count')['details']['name']=regex.APPLIED_PREFIX.replace('failed 0', 'failed 1')+'receipt'
        with self.assertRaises(ValueError): regex.validate_observations(PASS, data, self.path)

    def test_missing_extra_unexpanded_or_foreign_preview_rejected(self):
        for mutate in [lambda p: p['rows'].pop(), lambda p: p['rows'].append(p['rows'][1]),
                       lambda p: p['rows'][1].update(name='[\u2713] 17: item=alpha qty=12 \u2192 ${name}:$2'),
                       lambda p: p['rows'][0].update(name='[\u2713] Open document: foreign.txt'),
                       lambda p: p['rows'][1].update(control_type='ControlType.Edit'),
                       lambda p: p['status'].update(name=regex.PREVIEW_STATUS.replace('0 skipped', '1 skipped'))]:
            data=records(self.path);mutate(next(r for r in data if r['stage']=='regex capture preview')['details'])
            with self.assertRaises(ValueError): regex.validate_observations(PASS, data, self.path)

    def test_preview_mutation_partial_undo_and_encoding_changes_rejected(self):
        for stage, text in [('regex preview preserved text', regex.REPLACED),
                            ('regex replaced exact text', regex.REPLACED.replace('beta:07', 'item=beta\nqty=07')),
                            ('regex single Undo', regex.REPLACED),
                            ('regex single Undo', regex.INITIAL.replace('\n', '\r\n'))]:
            data=records(self.path);next(r for r in data if r['stage']==stage)['details']['text']=text
            with self.assertRaises(ValueError): regex.validate_observations(PASS, data, self.path)
        for stage in ['regex saved replacement bytes', 'regex restored original bytes']:
            for change in [{'base64': 'wrong'}, {'sha256': '0'*64}, {'bytes': True}]:
                data=records(self.path);next(r for r in data if r['stage']==stage)['details'].update(change)
                with self.assertRaises(ValueError): regex.validate_observations(PASS, data, self.path)

    def test_partial_failure_preserves_failure_without_demanding_later_observations(self):
        regex.validate_observations([dict(s, status='FAIL' if s['id']=='s1' else 'NOT_RUN') for s in PASS], [], self.path)
        with self.assertRaises(ValueError): regex.validate_observations(PASS, [], self.path)

    def test_real_adapter_routes_regex_and_gates_unsupported_cells(self):
        journey=next(j for j in runner.manifest(adapter.HERE/'journeys.json')['journeys'] if j['id']=='regex_transform')
        request={'journey':journey,'theme':'dark','dpi':'100','mode':'keyboard'}
        self.assertIn('regex_transform', adapter.IMPLEMENTED)
        self.assertIsNone(adapter.unavailable(request))
        self.assertIsNotNone(adapter.unavailable(dict(request, mode='screen_reader')))
        self.assertIsNotNone(adapter.unavailable(dict(request, dpi='100,150')))
        self.assertEqual(adapter.fixture_identity(request), regex.fixture()['identity'])
        self.assertIn('native_regex_transform.ps1', [Path(s['path']).name for s in adapter.source_identity()['auxiliary_sources']])

    def test_native_artifact_binding_and_exact_before_after_files(self):
        with tempfile.TemporaryDirectory() as directory:
            scratch=Path(directory).resolve();source=scratch/'fixture/input.txt';source.parent.mkdir()
            source.write_bytes(regex.INITIAL.encode('utf-8'))
            replaced=scratch/'regex-replaced.bin';replaced.write_bytes(regex.REPLACED.encode('utf-8'))
            capture=scratch/'regex-preview.png';capture.write_bytes(b'synthetic only')
            observed=scratch/'native-observations.json';observed.write_text(json.dumps({'records':records(source)}),encoding='utf-8')
            journey=next(j for j in runner.manifest(adapter.HERE/'journeys.json')['journeys'] if j['id']=='regex_transform')
            request={'journey':journey,'scratch':str(scratch),'binary_sha256':'a'*64}
            response=adapter.response_for(request,'PASS','synthetic only')
            response.update(binary_sha256='a'*64,fixture=regex.fixture()['identity'],cleanup={'editor_exited':True,'editor_exit_code':0},
                            artifacts=[{'path':str(p),'sha256':adapter.digest(p)} for p in (source,replaced,capture,observed)])
            def check():
                path=scratch/'native-response.json';path.write_text(json.dumps(response),encoding='utf-8')
                return adapter.checked_native_response(request,path)
            self.assertEqual(runner.observations(check(),journey),'PASS')
            pristine=copy.deepcopy(response)
            for file, good in [(source,regex.INITIAL),(replaced,regex.REPLACED)]:
                file.write_bytes(b'tampered')
                next(a for a in response['artifacts'] if a['path']==str(file))['sha256']=adapter.digest(file)
                with self.assertRaisesRegex(ValueError,'regex bytes mismatch'): check()
                file.write_bytes(good.encode('utf-8'));response=copy.deepcopy(pristine)
            response['artifacts']=[a for a in response['artifacts'] if a['path']!=str(capture)]
            with self.assertRaisesRegex(ValueError,'preview capture missing'): check()


if __name__ == '__main__': unittest.main()
