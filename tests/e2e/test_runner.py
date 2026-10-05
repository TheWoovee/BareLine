# SPDX-License-Identifier: MPL-2.0
"""Native harness regressions using synthetic responses and owned scratch data."""
import copy
from contextlib import redirect_stdout
import io
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import environment
import evidence_json
import native_adapter
import runner


def sample_environment():
    return {'os_family': 'windows', 'os_build': 'synthetic-19045', 'architecture': 'x64',
            'hardware': 'synthetic host', 'mode': 'keyboard', 'theme': 'dark', 'dpi': '100',
            'renderer': 'software', 'assistive_technology': 'none', 'build_mode': 'preview'}


class RunnerTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)


    def test_missing_observation_cannot_pass(self):
        journey = {"id": "fixture", "steps": [{"id": "a"}, {"id": "b"}]}
        response = {"schema_version": 1, "journey": "fixture", "steps": [
            {"id": "a", "status": "PASS", "observed": "value"}]}
        with self.assertRaises(ValueError):
            runner.observations(response, journey)
        response["steps"].append({"id": "b", "status": "NOT_RUN", "observed": "Unavailable"})
        self.assertEqual(runner.observations(response, journey), "FAIL")

    def test_duplicate_step_cannot_substitute_missing_step(self):
        journey = {"id": "fixture", "steps": [{"id": "a"}, {"id": "b"}]}
        step = {"id": "a", "status": "PASS", "observed": "value"}
        with self.assertRaises(ValueError):
            runner.observations({"schema_version": 1, "journey": "fixture",
                                 "steps": [step, copy.copy(step)]}, journey)

    def test_duplicate_journey_rejected(self):
        data = runner.read_json(runner.ROOT / "tests/e2e/journeys.json")
        data["journeys"][-1] = copy.deepcopy(data["journeys"][0])
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "manifest.json"
            runner.write_new(path, data)
            with self.assertRaises(ValueError):
                runner.manifest(path)

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

    def test_manifest_requires_tier_acceptance_cases_and_issue_ids(self):
        data = runner.read_json(runner.ROOT / 'tests/e2e/journeys.json')
        self.assertEqual(runner.tier(data, 'vm-only'), ['crash_recovery', 'extension_isolation', 'install_update_rollback'])
        self.assertIn('ui_regressions', runner.tier(data, 'ordinary'))
        regressions = next(row for row in data['journeys'] if row['id'] == 'ui_regressions')
        self.assertEqual([step.get('issue') for step in regressions['steps']],
                         ['ISSUE-005', 'ISSUE-008', 'U08', 'PR-T05', 'ISSUE-030'])
        for change in (lambda row: row.pop('tier'), lambda row: row.update(tier='lab'),
                       lambda row: row.pop('cases'), lambda row: row.update(cases=[]),
                       lambda row: row.update(cases=['AC-004-01', 'AC-004-01']), lambda row: row.update(cases=['FR-004']),
                       lambda row: row['steps'][0].update(issue='issue 5')):
            changed = copy.deepcopy(data)
            change(changed['journeys'][0])
            path = self.root / 'manifest.json'
            path.unlink(missing_ok=True)
            runner.write_new(path, changed)
            with self.subTest(journey=changed['journeys'][0]), self.assertRaises(ValueError):
                runner.manifest(path)

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

    def test_environment_identity_rejects_missing_unknown_and_ambiguous_values(self):
        original = sample_environment()
        self.assertEqual(environment.validate(original), original)
        for field in original:
            changed = dict(original); del changed[field]
            with self.subTest(missing=field), self.assertRaises(ValueError):
                environment.validate(changed)
        for change in ({'dp1': '100'}, {'dpi': 100}, {'renderer': 'auto'}, {'os_build': ' '},
                       {'mode': 'screen_reader'}, {'hardware': 'host\nother'}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                environment.validate(dict(original, **change))
        self.assertEqual(environment.validate(dict(original, mode='screen_reader', assistive_technology='Narrator synthetic'))['mode'], 'screen_reader')

    def test_environment_requires_captured_fields(self):
        request = sample_environment()
        for field in ('os_build', 'hardware', 'mode', 'theme', 'dpi'):
            for value in (None, '', ' ', 100):
                with self.subTest(field=field, value=value), self.assertRaisesRegex(ValueError, 'environment'):
                    runner.journey_environment(dict(request, **{field: value}))

    def test_json_reads_enforce_actual_byte_limits(self):
        path = self.root / 'bounded.json'
        with patch.object(evidence_json, 'MAX_JSON_BYTES', 32):
            path.write_bytes(b'{"x":1}' + b' ' * 25)
            self.assertEqual(runner.read_json(path), {'x': 1})
            path.write_bytes(b'{"x":1}' + b' ' * 26)
            with self.assertRaisesRegex(ValueError, 'exceeds'):
                runner.read_json(path)


if __name__ == "__main__":
    unittest.main()
