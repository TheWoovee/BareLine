# SPDX-License-Identifier: MPL-2.0
"""Focused negative evidence regressions; no native product acceptance claims."""
import copy
from contextlib import redirect_stdout
import io
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch
import native_adapter
import evidence_json
import runner
import test_runner

class ReadinessDefectTests(unittest.TestCase):
    _write_receipt = test_runner.RunnerTests._write_receipt
    _qualification_fixture = test_runner.RunnerTests._qualification_fixture

    def setUp(self):
        temporary = tempfile.TemporaryDirectory(dir=runner.ROOT / 'target')
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)

    def test_evidence_reads_enforce_actual_byte_limits(self):
        path = self.root / 'bounded.json'
        with patch.object(evidence_json, 'MAX_JSON_BYTES', 32, create=True):
            path.write_bytes(b'{"x":1}' + b' ' * 25)
            self.assertEqual(runner.read_json(path), {'x': 1})
            path.write_bytes(b'{"x":1}' + b' ' * 26)
            with self.subTest(kind='JSON'), self.assertRaisesRegex(ValueError, 'exceeds'):
                runner.read_json(path)
        identity = {'available': True, 'head': 'a' * 40, 'working_tree_dirty': False, 'source_manifest_sha256': 'b' * 64}
        for stream in ('stdout', 'stderr'):
            receipt_path = self._write_receipt(self.root, stream, '', identity)
            receipt = runner.read_json(receipt_path)
            raw_path = Path(receipt[stream]['path'])
            for size in (32, 33):
                raw_path.write_bytes(b'x' * size)
                receipt[stream]['sha256'] = runner.digest(raw_path)
                receipt_path.write_text(json.dumps(receipt), encoding='utf-8')
                with patch.object(runner, 'MAX_LOG_BYTES', 32, create=True):
                    if size == 32:
                        runner.checked_test_receipt(receipt_path)
                    else:
                        with self.subTest(kind=stream), self.assertRaisesRegex(ValueError, 'exceeds'):
                            runner.checked_test_receipt(receipt_path)

    def test_capture_requires_one_unambiguous_result_binding(self):
        artifact = self.root / 'result.json'
        sha = 'a' * 64
        good = json.dumps({'evidence_result': str(artifact), 'sha256': sha}) + '\n'
        wrong = json.dumps({'evidence_result': str(self.root / 'other.json'), 'sha256': 'b' * 64}) + '\n'
        legacy = f'journey: evidence={artifact}\njourney: evidence_sha256={sha}\n'
        for raw in (good, legacy):
            runner.require_capture_binding(('ordinary output\n' + raw).encode(), artifact, sha, 'journey-result')
        for raw in (wrong + good, good + wrong, good + good, good + legacy, good + 'journey: evidence=other.json\n', good + '{"evidence_result":"other.json","sha256":"bad","sha256":"bad"}\n'):
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                runner.require_capture_binding(raw.encode(), artifact, sha, 'journey-result')

    def test_environment_labels_are_preserved_and_cannot_be_rewritten(self):
        args, _, _ = self._qualification_fixture(self.root)
        runner.adapt(args)
        original = runner.read_json(args.output)
        request = runner.read_json(args.journey_result)['request']
        fields = ('os_build', 'hardware', 'mode', 'theme', 'dpi')
        for field in fields:
            with self.subTest(preserved=field):
                self.assertEqual(original['evidence'][0].get(field), request[field])
        for field in fields:
            value = copy.deepcopy(original)
            value['evidence'][0][field] = 'changed environment'
            args.output.write_text(json.dumps(value), encoding='utf-8')
            resolve = SimpleNamespace(index=runner.ROOT / 'docs/parity/index.json', inventory=None, inventory_receipt=None, evidence=[args.output], output=self.root / (field + '-report.json'))
            with self.subTest(changed=field), self.assertRaisesRegex(ValueError, 'environment'):
                runner.resolve(resolve)

    def test_reviewer_identity_rejects_spelling_variants_in_both_import_paths(self):
        args, _, _ = self._qualification_fixture(self.root)
        runner.adapt(args)
        original = runner.read_json(args.output)
        args.output = self.root / 'invalid-review.json'
        for reviewer in (' ALICE SMITH ', 'Alice   Smith', 'alice smith', 'Ａｌｉｃｅ Smith'):
            args.implementer, args.reviewer = ('Alice Smith', reviewer)
            with self.subTest(imported=reviewer), self.assertRaisesRegex(ValueError, 'Independent reviewer'):
                runner.adapt(args)
            changed = copy.deepcopy(original)
            changed['evidence'][0].update(implementer=args.implementer, reviewer=reviewer)
            args.output.write_text(json.dumps(changed), encoding='utf-8')
            resolve = SimpleNamespace(index=runner.ROOT / 'docs/parity/index.json', inventory=None, evidence=[args.output], output=None)
            with self.subTest(resolved=reviewer), self.assertRaisesRegex(ValueError, 'Independent reviewer'):
                runner.resolve(resolve)
            args.output.unlink()

    def test_environment_requires_captured_fields_and_rejects_cli_overrides(self):
        args, _, _ = self._qualification_fixture(self.root)
        request = runner.read_json(args.journey_result)['request']
        for field in ('os_build', 'hardware', 'mode', 'theme', 'dpi'):
            for value in (None, '', ' ', 100):
                changed = dict(request, **{field: value})
                with self.subTest(field=field, value=value), self.assertRaisesRegex(ValueError, 'environment'):
                    runner.journey_environment(changed)
        args.os_build = 'invented later'
        with self.assertRaisesRegex(ValueError, 'CLI environment'):
            runner.adapt(args)

    def test_unbound_or_changed_fixtures_cannot_be_attached_after_execution(self):
        for kind in ('unrelated', 'changed'):
            root = self.root / kind
            root.mkdir()
            args, _, _ = self._qualification_fixture(root)
            if kind == 'unrelated':
                other = root / 'never-used.txt'
                other.write_text('not executed', encoding='utf-8')
                args.fixture = [other]
            else:
                args.fixture[0].write_text('changed after captured result', encoding='utf-8')
            with self.subTest(kind=kind), self.assertRaisesRegex(ValueError, '[Ff]ixture|[Aa]rtifact'):
                runner.adapt(args)
        root = self.root / 'bound'
        root.mkdir()
        args, _, _ = self._qualification_fixture(root)
        self.assertEqual(runner.adapt(args), 0)

    def test_changed_executable_cannot_pass_after_adapter_cleanup(self):
        journey = runner.manifest(runner.ROOT / 'tests/e2e/journeys.json')['journeys'][0]
        executable = self.root / 'synthetic.exe'
        executable.write_bytes(b'initial; never executed')
        args = SimpleNamespace(manifest=None, journey=journey['id'], commit='a' * 40, reviewer='synthetic', os_build='synthetic', hardware='synthetic', executable=str(executable), adapter=[str(executable)], mode='keyboard', theme='dark', dpi='100')
        for change in (False, True):
            executable.write_bytes(b'initial; never executed')
            child = Mock()
            child.poll_exit_code.return_value = 0
            if change:
                child.close.side_effect = lambda: executable.write_bytes(b'changed during cleanup')

            def launch(argv, cwd):
                request = runner.read_json(Path(argv[-1]))
                response = native_adapter.response_for(request, 'PASS', 'synthetic only')
                runner.write_new(Path(request['response']), response)
                return child
            output = io.StringIO()
            with patch.object(runner, 'ROOT', self.root), patch.object(runner, 'os', SimpleNamespace(name='nt')), patch.object(runner, 'manifest', return_value={'journeys': [journey]}), patch('windows_process_metrics.OwnedProcessTree', side_effect=launch), redirect_stdout(output):
                code = runner.run(args)
            child.close.assert_called_once()
            result = runner.read_json(json.loads(output.getvalue())['evidence_result'])
            with self.subTest(changed=change):
                self.assertEqual(code, 1 if change else 0)
                self.assertEqual(result['status'], 'FAIL' if change else 'PASS')

    def test_result_must_match_requested_and_captured_journey(self):
        args, _, _ = self._qualification_fixture(self.root)
        original = runner.read_json(args.journey_result)
        with self.assertRaisesRegex(ValueError, 'Requested journey'):
            runner.checked_journey_result(args.journey_result, 'regex_transform')
        for change in ('missing', 'action', 'expected', 'schema'):
            result = copy.deepcopy(original)
            if change == 'missing':
                del result['request']['journey']
            elif change == 'schema':
                result['request']['schema_version'] = True
            else:
                result['request']['journey']['steps'][0][change] = 'different procedure'
            args.journey_result.write_text(json.dumps(result), encoding='utf-8')
            with self.subTest(change=change), self.assertRaisesRegex(ValueError, 'Captured journey'):
                runner.checked_journey_result(args.journey_result)
        args.journey_result.write_text(json.dumps(original), encoding='utf-8')
        runner.checked_journey_result(args.journey_result, 'column_multi_cursor')

    def test_receipt_accounting_requires_real_integers(self):
        identity = {'available': True, 'head': 'a' * 40, 'working_tree_dirty': False, 'source_manifest_sha256': 'b' * 64}
        path = self._write_receipt(self.root, 'receipt', 'synthetic', identity)
        original = runner.read_json(path)
        for key, wrong in [('schema_version', True), ('schema_version', 1.0), ('exit_code', False), ('exit_code', 0.0), ('top_level_total', True), ('top_level_passed', 1.0), ('top_level_failed', False)]:
            value = copy.deepcopy(original)
            value[key] = wrong
            path.write_text(json.dumps(value), encoding='utf-8')
            with self.subTest(key=key, wrong=wrong), self.assertRaises(ValueError):
                runner.checked_test_receipt(path)
        path.write_text(json.dumps(original), encoding='utf-8')
        runner.checked_test_receipt(path)

    def test_response_schema_requires_real_integer(self):
        journey = runner.manifest(runner.ROOT / 'tests/e2e/journeys.json')['journeys'][0]
        response = native_adapter.response_for({'journey': journey}, 'PASS', 'synthetic')
        for wrong in (True, 1.0):
            response['schema_version'] = wrong
            with self.subTest(wrong=wrong), self.assertRaises(ValueError):
                runner.observations(response, journey)

    def test_duplicate_json_fields_are_rejected_by_both_evidence_readers(self):
        path = self.root / 'ambiguous.json'
        for raw in ['{"status":"FAIL","status":"PASS"}', '{"source":{"sha256":"first","sha256":"second"}}', '{"schema_version":0,"schema_version":1}']:
            path.write_text(raw, encoding='utf-8')
            for reader in (runner.read_json, native_adapter.read_bounded):
                with self.subTest(raw=raw, reader=reader.__name__):
                    with self.assertRaisesRegex(ValueError, 'Duplicate JSON'):
                        reader(path)
        path.write_text('{"status":"FAIL","steps":[{"status":"PASS"}]}', encoding='utf-8')
        self.assertEqual(runner.read_json(path)['status'], 'FAIL')
if __name__ == '__main__':
    unittest.main()
