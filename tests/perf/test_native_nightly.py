# SPDX-License-Identifier: MPL-2.0
"""Pure contract tests; never launch an editor or invoke a benchmark adapter."""
from pathlib import Path
import tempfile
import unittest

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
            native_nightly.run('', '', 'does-not-exist.exe', target)
            report = perf_suite.read_json(target / 'native-report.json')
            self.assertEqual(len(report['coverage']), len(native_nightly.required_cases()))
            self.assertTrue(all(case['status'] == 'unavailable' and case['reason'] for case in report['coverage']))
            self.assertEqual(report['rows'], [])
            self.assertFalse(report['claims_eligible'])

    def test_failed_or_incomplete_samples_never_enter_rolling_baseline(self):
        report = {'observations': [{'application': 'bareline', 'metric': 'operation_us',
                  'sample_count': 3, 'p50': 100, 'p95': 110}], 'failures': [], 'missing_trials': 0}
        rows = native_nightly.regression_rows(report, 'scroll', 'software', 3)
        self.assertEqual(rows[0]['scenario'], 'scroll@software')
        self.assertEqual(native_nightly.regression_rows({**report, 'missing_trials': 1}, 'scroll', 'software', 3), [])
        self.assertEqual(native_nightly.regression_rows({**report, 'failures': ['timeout']}, 'scroll', 'software', 3), [])
        self.assertEqual(native_nightly.regression_rows(report, 'scroll', 'software', 4), [])

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
