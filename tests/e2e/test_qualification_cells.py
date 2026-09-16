# SPDX-License-Identifier: MPL-2.0
"""Synthetic coverage-contract checks; never native acceptance evidence."""
import copy
import json
import subprocess
import sys
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import qualification_cells as cells
import native_adapter
import runner
import test_runner


def sample_environment():
    return {'os_family': 'windows', 'os_build': 'synthetic-19045', 'architecture': 'x64',
            'hardware': 'synthetic host', 'mode': 'keyboard', 'theme': 'dark', 'dpi': '100',
            'renderer': 'software', 'assistive_technology': 'none', 'build_mode': 'preview'}


def sample_matrix():
    return {'schema_version': 1, 'kind': 'qualification_environment_matrix',
            'review': {'implementer': 'synthetic author', 'reviewer': 'synthetic reviewer'},
            'cells': [{'id': 'win10', 'environment': sample_environment(), 'binary_sha256': 'c'*64,
                       'source_identity': {'available': True, 'head': 'a'*40, 'working_tree_dirty': True,
                                           'source_manifest_sha256': 'b'*64}}],
            'requirements': {'AC-006-01': ['win10']}, 'exclusions': []}


class CellTests(unittest.TestCase):
    _write_receipt = test_runner.RunnerTests._write_receipt
    _qualification_fixture = test_runner.RunnerTests._qualification_fixture

    def setUp(self):
        temporary = tempfile.TemporaryDirectory(dir=runner.ROOT / 'target')
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)

    def test_06_environment_identity_rejects_missing_unknown_and_ambiguous_values(self):
        original = sample_environment()
        self.assertEqual(cells.environment(original), original)
        for field in original:
            changed = dict(original); del changed[field]
            with self.subTest(missing=field), self.assertRaises(ValueError):
                cells.environment(changed)
        for change in ({'dp1': '100'}, {'dpi': 100}, {'renderer': 'auto'}, {'os_build': ' '},
                       {'mode': 'screen_reader'}, {'hardware': 'host\nother'}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                cells.environment(dict(original, **change))
        self.assertEqual(cells.environment(dict(original, mode='screen_reader', assistive_technology='Narrator synthetic'))['mode'], 'screen_reader')

    def test_07_matrix_requires_complete_cases_unique_cells_and_artifact_identity(self):
        original = sample_matrix()
        self.assertEqual(cells.matrix(original, {'AC-006-01'}), original)
        with patch.object(cells, 'MAX_REQUIRED_CELLS', 0), self.assertRaisesRegex(ValueError, 'Too many required'):
            cells.matrix(original, {'AC-006-01'})
        for change in ('missing_case', 'extra_case', 'duplicate_cell', 'unknown_cell', 'duplicate_required', 'bad_binary', 'bad_source', 'schema'):
            document = copy.deepcopy(original)
            if change == 'missing_case': document['requirements'] = {}
            elif change == 'extra_case': document['requirements']['AC-006-02'] = ['win10']
            elif change == 'duplicate_cell': document['cells'].append(copy.deepcopy(document['cells'][0]))
            elif change == 'unknown_cell': document['requirements']['AC-006-01'] = ['missing']
            elif change == 'duplicate_required': document['requirements']['AC-006-01'].append('win10')
            elif change == 'bad_binary': document['cells'][0]['binary_sha256'] = 'unverified'
            elif change == 'bad_source': document['cells'][0]['source_identity']['working_tree_dirty'] = 1
            else: document['schema_version'] = True
            with self.subTest(change=change), self.assertRaises(ValueError):
                cells.matrix(document, {'AC-006-01'})

    def test_08_case_can_repeat_across_required_cells_but_not_within_one_cell(self):
        document = sample_matrix()
        second = copy.deepcopy(document['cells'][0]); second['id'] = 'win11'
        second['environment']['os_build'] = 'synthetic-22631'
        document['cells'].append(second)
        document['requirements']['AC-006-01'].append('win11')
        records = [{'id': 'AC-006-01', 'cell_id': cell} for cell in ('win10', 'win11')]
        self.assertEqual(len(cells.index_records(records, document)), 2)
        for extra in (records[0], {'id': 'AC-006-01'}, {'id': 'AC-006-01', 'cell_id': 'unknown'},
                      {'id': 'unknown', 'cell_id': 'win10'}):
            with self.subTest(extra=extra), self.assertRaises(ValueError):
                cells.index_records(records + [extra], document)

    def test_09_cell_identity_cannot_be_relabelled(self):
        cell = sample_matrix()['cells'][0]
        record = dict(cell['environment'], source_identity=cell['source_identity'], binary_sha256=cell['binary_sha256'])
        cells.require_match(record, cell)
        for field, value in (('os_build', 'synthetic-other'), ('renderer', 'hardware'), ('binary_sha256', 'd'*64),
                             ('source_identity', dict(cell['source_identity'], source_manifest_sha256='e'*64))):
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, 'required cell'):
                cells.require_match(dict(record, **{field: value}), cell)
        request = dict(cell['environment'], qualification_environment=cell['environment'])
        self.assertEqual(runner.journey_environment(request), cell['environment'])
        request['dpi'] = '200'
        with self.assertRaisesRegex(ValueError, 'contradicts'):
            runner.journey_environment(request)

    def test_10_failed_and_unrun_captures_survive_without_becoming_pass(self):
        for status in ('FAIL', 'NOT_RUN'):
            root = self.root / status; root.mkdir()
            args, _, _ = self._qualification_fixture(root)
            result = runner.read_json(args.journey_result)
            result['status'] = status
            result['adapter_exit_code'] = 7 if status == 'FAIL' else None
            for step in result['steps']:
                step['status'] = status
                step['observed'] = 'synthetic failure' if status == 'FAIL' else 'synthetic step was not executed'
                if status == 'NOT_RUN':
                    step['artifacts'] = []
            if status == 'NOT_RUN':
                args.fixture = []
            args.journey_result.write_text(json.dumps(result), encoding='utf-8')
            receipt = runner.read_json(args.test_receipt)
            stdout = Path(receipt['stdout']['path'])
            stdout.write_text(json.dumps({'evidence_result': str(args.journey_result), 'sha256': runner.digest(args.journey_result)}), encoding='utf-8')
            receipt.update(exit_code=1, top_level_passed=0, top_level_failed=1)
            receipt['stdout']['sha256'] = runner.digest(stdout)
            args.test_receipt.write_text(json.dumps(receipt), encoding='utf-8')
            runner.adapt(args)
            bundle = runner.read_json(args.output)
            self.assertEqual(bundle['evidence'][0]['status'], status)
            report = root / 'report.json'
            resolve = SimpleNamespace(index=runner.ROOT / 'docs/parity/index.json', inventory=None,
                                      evidence=[args.output], output=report)
            self.assertEqual(runner.resolve(resolve), 1)
            self.assertEqual(runner.read_json(report)['resolved']['AC-006-01'], status)
            bundle['evidence'][0]['status'] = 'PASS'
            args.output.write_text(json.dumps(bundle), encoding='utf-8')
            with self.assertRaises(ValueError):
                runner.resolve(resolve)

    def test_11_all_required_cells_must_complete_and_negative_results_stay_visible(self):
        matrix = sample_matrix()
        matrix['requirements']['AC-006-01'].append('win11')
        for second in ('MISSING', 'FAIL', 'NOT_RUN', 'PASS'):
            statuses = {('AC-006-01', 'win10'): 'PASS'}
            if second != 'MISSING': statuses[('AC-006-01', 'win11')] = second
            resolved, rows, unresolved = cells.coverage(matrix, statuses)
            self.assertEqual(resolved['AC-006-01'], second)
            self.assertEqual(len(rows), 2)
            self.assertEqual(bool(unresolved), second != 'PASS')
        # Exercise real adaptation and resolver wiring, not only the aggregator.
        bundles = []
        for name, build in (('win10', 'synthetic-19045'), ('win11', 'synthetic-22631')):
            root = self.root / name; root.mkdir()
            args, identity, binary = self._qualification_fixture(root)
            env = dict(sample_environment(), os_build=build)
            result = runner.read_json(args.journey_result)
            result['request'].update({key: env[key] for key in ('os_build', 'hardware', 'mode', 'theme', 'dpi')})
            result['request']['qualification_environment'] = env
            args.journey_result.write_text(json.dumps(result), encoding='utf-8')
            receipt = runner.read_json(args.test_receipt)
            stdout = Path(receipt['stdout']['path'])
            stdout.write_text(json.dumps({'evidence_result': str(args.journey_result), 'sha256': runner.digest(args.journey_result)}), encoding='utf-8')
            receipt['stdout']['sha256'] = runner.digest(stdout)
            args.test_receipt.write_text(json.dumps(receipt), encoding='utf-8')
            args.cell = name
            runner.adapt(args); bundles.append(args.output)
            definition = {'id': name, 'environment': env, 'source_identity': identity, 'binary_sha256': runner.digest(binary)}
            if name == 'win10': matrix['cells'] = [definition]
            else: matrix['cells'].append(definition)
        atomic = set(runner.re.findall(r'AC-\d{3}-\d{2}', (runner.ROOT / 'docs/blueprint/10_ACCEPTANCE_AND_TRACEABILITY.md').read_text(encoding='utf-8')))
        matrix['requirements'].update({key: ['win10'] for key in atomic if key != 'AC-006-01'})
        path = self.root / 'matrix.json'; path.write_text(json.dumps(matrix), encoding='utf-8')
        for count in (1, 2):
            output = self.root / f'coverage-{count}.json'
            runner.resolve(SimpleNamespace(index=runner.ROOT / 'docs/parity/index.json', inventory=None,
                                           matrix=path, evidence=bundles[:count], output=output))
            report = runner.read_json(output)
            self.assertEqual(report['resolved']['AC-006-01'], 'MISSING' if count == 1 else 'PASS')
            self.assertFalse(report['evidence_complete'])  # All other acceptance work is still absent.

    def test_12_cli_requires_exact_per_cell_exclusion_authority(self):
        request = dict(sample_environment(), journey={'id': 'plain_text'}, qualification_environment=sample_environment())
        self.assertIsNone(native_adapter.unavailable(request))
        request['qualification_environment']['renderer'] = 'hardware'
        self.assertIn('NOT_RUN', native_adapter.unavailable(request))
        matrix = sample_matrix()
        atomic = set(runner.re.findall(r'AC-\d{3}-\d{2}', (runner.ROOT / 'docs/blueprint/10_ACCEPTANCE_AND_TRACEABILITY.md').read_text(encoding='utf-8')))
        matrix['requirements'] = {key: ['win10'] for key in atomic}
        authority = self.root / 'authority.md'; authority.write_text('Synthetic exclusion contract only', encoding='utf-8')
        exclusion = {'id': 'AC-006-01', 'cell_id': 'win10', 'reason': 'synthetic control',
                     'authority': authority.relative_to(runner.ROOT).as_posix(), 'authority_sha256': runner.digest(authority)}
        matrix['exclusions'] = [exclusion]
        matrix_path = self.root / 'matrix.json'; matrix_path.write_text(json.dumps(matrix), encoding='utf-8')
        bundle = self.root / 'bundle.json'
        bundle.write_text(json.dumps({'schema_version': 1, 'evidence': [dict(exclusion, excluded=True)]}), encoding='utf-8')
        report = self.root / 'report.json'
        command = [sys.executable, str(runner.ROOT / 'tests/e2e/runner.py'), 'resolve', '--matrix', str(matrix_path),
                   '--evidence', str(bundle), '--output', str(report)]
        result = subprocess.run(command, cwd=runner.ROOT, capture_output=True, text=True)
        self.assertEqual(result.returncode, 1, result.stderr)  # Other required work is missing.
        self.assertEqual(runner.read_json(report)['resolved']['AC-006-01'], 'EXCLUDED')
        with self.assertRaisesRegex(ValueError, 'reviewed matrix'):
            cells.require_exclusion(dict(exclusion, reason='rewritten', excluded=True), matrix)
        authority.write_text('changed authority', encoding='utf-8')
        result = subprocess.run(command, cwd=runner.ROOT, capture_output=True, text=True)
        self.assertEqual(result.returncode, 2)
        self.assertIn('authority bytes changed', result.stderr)


if __name__ == '__main__':
    unittest.main()
