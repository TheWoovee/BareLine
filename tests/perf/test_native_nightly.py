# SPDX-License-Identifier: MPL-2.0
"""Pure contract tests; never launch an editor or invoke a benchmark adapter."""
from pathlib import Path
import tempfile
import unittest
from unittest import mock

from disk_metrics import DiskSampler, snapshot, isolated_environment
import native_nightly
import perf_suite


class NativeContracts(unittest.TestCase):
    def test_full_registry_has_both_required_renderer_modes(self):
        cases = native_nightly.required_cases()
        self.assertEqual(set(name for name, _ in cases), set(perf_suite.SCENARIOS))
        self.assertEqual(len(cases), len(set(cases)))
        for name in ('cold_launch', 'warm_launch', 'empty_idle', 'scroll'):
            self.assertIn((name, 'hardware'), cases)
            self.assertIn((name, 'software'), cases)

    def test_missing_plan_records_every_case_without_launch(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / 'run'
            self.assertEqual(native_nightly.run('', '', 'does-not-exist.exe', target), 1)
            report = perf_suite.read_json(target / 'native-report.json')
            self.assertEqual(report['schema_version'], 2)
            self.assertEqual(len(report['coverage']), len(native_nightly.required_cases()))
            self.assertTrue(all(case['status'] == 'unavailable' and case['reason'] for case in report['coverage']))
            self.assertEqual(len({case['id'] for case in report['coverage']}), len(report['coverage']))
            self.assertIsNone(report['provenance']['manifest']['application']['sha256'])
            self.assertIn('comparator application is absent', report['ineligibility_reasons'])
            self.assertEqual(report['rows'], [])
            self.assertFalse(report['claims_eligible'])

    def test_cli_propagates_unavailable_qualification_as_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / 'run'
            argv = ['native_nightly.py', '--application', 'does-not-exist.exe',
                    '--destination', str(target)]
            with mock.patch('sys.argv', argv):
                self.assertEqual(native_nightly.main(), 1)
            self.assertTrue((target / 'native-report.json').is_file())

    def test_failed_or_incomplete_samples_never_enter_rolling_baseline(self):
        identity = {'available': True, 'head': 'fixture', 'working_tree_dirty': False,
                    'source_manifest_sha256': 'a' * 64}
        report = {'provenance': {'source_before': identity, 'source_after': identity,
                  'source_changed_during_run': False},
                  'observations': [{'application': 'bareline', 'metric': 'operation_us',
                  'sample_count': 3, 'p50': 100, 'p95': 110, 'status': 'measured'}],
                  'failures': [], 'missing_trials': 0}
        rows = native_nightly.regression_rows(report, 'scroll', 'software', 3)
        self.assertEqual(rows[0]['scenario'], 'scroll@software')
        self.assertEqual(native_nightly.regression_rows({**report, 'missing_trials': 1}, 'scroll', 'software', 3), [])
        self.assertEqual(native_nightly.regression_rows({**report, 'failures': ['timeout']}, 'scroll', 'software', 3), [])
        self.assertEqual(native_nightly.regression_rows(report, 'scroll', 'software', 4), [])
        unqualified = {**report, 'observations': [{**report['observations'][0], 'status': 'unqualified'}]}
        self.assertEqual(native_nightly.regression_rows(unqualified, 'scroll', 'software', 3), [])

    def test_source_change_rejects_rows_and_success_exit(self):
        before = {'available': True, 'head': 'fixture', 'working_tree_dirty': False,
                  'source_manifest_sha256': 'a' * 64}
        after = {**before, 'source_manifest_sha256': 'b' * 64}
        report = {'provenance': {'source_before': before, 'source_after': after,
                  'source_changed_during_run': True}, 'observations': [],
                  'failures': [], 'missing_trials': 0}
        self.assertEqual(native_nightly.regression_rows(report, 'scroll', 'hardware', 3), [])
        self.assertEqual(native_nightly.qualification_exit_code(before, after,
                         [{'status': 'measured'}]), 1)

    def test_disk_peak_growth_and_release_use_actual_owned_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'extensions').mkdir()
            package = root / 'extensions' / 'component'
            package.write_bytes(b'x' * 10)
            sample = DiskSampler(root)
            (root / 'recovery').mkdir()
            journal = root / 'recovery' / 'journal'
            journal.write_bytes(b'x' * 23)
            (root / 'temporary').mkdir()
            spill = root / 'temporary' / 'spill'
            spill.write_bytes(b'x' * 37)
            sample.observe()
            spill.unlink()
            package.unlink()
            sample.stop()
            metrics = sample.metrics()
            self.assertEqual(metrics['temporary_disk_peak_growth_bytes'], 37)
            self.assertEqual(metrics['temporary_disk_after_bytes'], 0)
            self.assertEqual(metrics['recovery_disk_peak_growth_bytes'], 23)
            self.assertEqual(metrics['extensions_disk_released_bytes'], 10)

    def test_disk_quota_is_not_a_partial_total(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'one').write_bytes(b'x')
            (root / 'two').write_bytes(b'x')
            with self.assertRaises(ValueError):
                snapshot(root, max_entries=1)

    def test_child_environment_roots_are_explicitly_inside_owned_tree(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            environment = isolated_environment(root)
            for key in ('TEMP', 'TMP', 'LOCALAPPDATA', 'APPDATA'):
                self.assertTrue(Path(environment[key]).is_relative_to(root))


if __name__ == '__main__':
    unittest.main()
