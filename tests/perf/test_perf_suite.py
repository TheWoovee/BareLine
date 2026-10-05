# SPDX-License-Identifier: MPL-2.0
"""Focused harness regressions; run only at the coordinated verification gate."""
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import tempfile
import unittest

import native_nightly
import perf_suite as suite


def qualification_environment(font):
    return {
        'hardware': {'cpu_model': 'controlled fixture CPU', 'logical_cpus': os.cpu_count(),
                     'physical_memory_bytes': 1, 'storage_model': 'controlled fixture storage'},
        'os': {'name': platform.system(), 'release': platform.release(), 'version': platform.version(),
               'build': platform.version().split('.')[-1], 'architecture': platform.machine()},
        'power': {'mode': 'controlled fixture mode', 'source': 'controlled fixture source'},
        'display': {'dpi': 96, 'scale_percent': 100},
        'fonts': [{'family': 'fixture', 'version': '1', 'path': str(font), 'sha256': suite.digest(font)}],
        'editor': {'theme': 'light', 'wrap': 'off', 'syntax': 'plain text'},
        'acquisition': {
            'setup_compilation': {'status': 'not_applicable', 'reason': 'inert Python driver'},
            'download': {'status': 'not_applicable', 'reason': 'local test fixture'},
        },
        'cold_cache': {'method': 'not applicable', 'plan_sha256': None},
    }


def init_source_repository(root):
    root.mkdir()
    tracked = root / 'tracked.txt'
    tracked.write_text('original', encoding='utf-8')
    for arguments in (('init',), ('add', 'tracked.txt'),
                      ('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                       'commit', '-m', 'fixture')):
        subprocess.run(['git', *arguments], cwd=root, check=True, capture_output=True)
    return tracked


def inert_manifest(root, mutate=False):
    source = root / 'source'
    tracked = init_source_repository(source)
    font = root / 'font.bin'; font.write_bytes(b'font fixture')
    driver = root / 'driver.py'
    driver.write_text("import json, pathlib, sys\nif len(sys.argv) > 1: pathlib.Path(sys.argv[1]).write_text('changed', encoding='utf-8')\nprint(json.dumps({'event':'measurement','metrics':{'latency_us':10}}))\n", encoding='utf-8')
    identity = suite.t09_source_identity(source)
    python = str(Path(sys.executable).resolve())
    driver_record = {'argv': [python, str(driver)] + ([str(tracked)] if mutate else []),
                     'sha256': suite.digest(python),
                     'pinned_files': [{'path': str(driver), 'sha256': suite.digest(driver)}]}
    app = {'executable': python, 'sha256': suite.digest(python), 'version': platform.python_version(),
           'settings': {'config': 'inert'}}
    return {'schema_version': 2, 'series': 'local', 'machine_id': platform.node(),
            'configuration': 'controlled-inert-driver', 'repetitions': 1,
            'qualification_environment': qualification_environment(font),
            'source_identity': {'algorithm': 't09-run-test-evidence-v1', 'root': str(source),
                                'excluded_paths': [], 'before': identity},
            'applications': {'bareline': app, 'notepadpp': {**app, 'settings': {'plugins_disabled': True}}},
            'scenarios': [{'name': 'save', 'timeout_seconds': 5, 'cache_state': 'controlled inert',
                           'renderer': 'hardware', 'fixtures': [], 'comparable_metrics': ['latency_us'],
                           'drivers': {'bareline': driver_record, 'notepadpp': driver_record}}]}


class EvidenceTests(unittest.TestCase):
    def test_qualification_environment_requires_structured_reproducibility_pins(self):
        environment = {
            'hardware': {'cpu_model': 'fixture cpu', 'logical_cpus': 1,
                         'physical_memory_bytes': 1, 'storage_model': 'fixture disk'},
            'os': {'name': 'Windows', 'release': 'fixture', 'version': 'fixture',
                   'build': '1', 'architecture': 'x64'},
            'power': {'mode': 'balanced', 'source': 'AC'},
            'display': {'dpi': 96, 'scale_percent': 100},
            'fonts': [],
            'editor': {'theme': 'light', 'wrap': 'off', 'syntax': 'plain text'},
            'acquisition': {
                'setup_compilation': {'status': 'unavailable', 'reason': 'fixture'},
                'download': {'status': 'not_applicable', 'reason': 'offline fixture'},
            },
            'cold_cache': {'method': 'pinned external receipt'},
        }
        with self.assertRaisesRegex(ValueError, 'fonts'):
            suite.validate_qualification_environment(environment)

    def test_environment_rejects_nonfinite_display_and_untyped_hashes(self):
        with tempfile.TemporaryDirectory() as directory:
            font = Path(directory) / 'font.bin'; font.write_bytes(b'font')
            environment = qualification_environment(font)
            environment['display']['dpi'] = float('nan')
            with self.assertRaisesRegex(ValueError, 'dpi'):
                suite.validate_qualification_environment(environment)
            environment = qualification_environment(font)
            environment['fonts'][0]['sha256'] = 7
            with self.assertRaisesRegex(ValueError, 'font'):
                suite.validate_qualification_environment(environment)

    def test_cold_cache_method_and_scenario_use_the_same_plan(self):
        manifest = {'qualification_environment': {'cold_cache': {'plan_sha256': 'a' * 64}},
                    'scenarios': [{'name': 'cold_launch', 'cache_plan_sha256': 'b' * 64}]}
        with self.assertRaisesRegex(ValueError, 'cold launch plan'):
            suite.validate_cold_cache_pins(manifest)

    def test_timeout_and_failing_child_retain_diagnostics(self):
        result = suite.capture([sys.executable, "-c", "import sys; print('receipt', flush=True); sys.exit(3)"], None, 2)
        self.assertEqual(result["status"], "failed")
        self.assertEqual(result["exit_code"], 3)
        self.assertIn("receipt", result["stdout"])
        result = suite.capture([sys.executable, "-c", "import time; print('started', flush=True); time.sleep(10)"], None, .1)
        self.assertEqual(result["status"], "timeout")
        self.assertIn("started", result["stdout"])

    def test_output_flood_is_bounded_and_does_not_deadlock(self):
        result = suite.capture([sys.executable, "-c", "import sys; sys.stderr.write('x'*2000000); sys.stderr.flush()"], None, 3)
        self.assertLessEqual(len(result["stderr"]), suite.LIMIT)
        self.assertNotEqual(result["status"], "timeout")

    def test_incomplete_pair_cannot_improve_reported_ratio(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            suite.write_new(root / "provenance.json", {"manifest": {"repetitions": 2, "series": "local", "scenarios": [{"name": "edit_to_paint", "comparable_metrics": ["latency_us"]}]}, "missing_scenarios": []})
            for pair, app, position, value in [(0, "bareline", 0, 10), (0, "notepadpp", 1, 20), (1, "bareline", 1, 1)]:
                suite.write_new(root / f"edit_to_paint-{pair}-{app}.json", {"scenario": "edit_to_paint", "pair": pair, "application": app, "position": position, "metrics": {"latency_us": value}, "status": "ok"})
            suite.report(root, root / "report.out")
            report = suite.read_json(root / "report.out")
            self.assertEqual(report["rows"][0]["p50_ratio"], .5)
            self.assertEqual(report["rows"][0]["excluded_pairs"], [1])
            self.assertEqual(report['rows'][0]['status'], 'unqualified')
            self.assertEqual([item['status'] for item in report['coverage']].count('not_run'), 1)
            self.assertIsNone(next(item for item in report['coverage'] if item['status'] == 'not_run')['artifact'])
            self.assertFalse(report["claims_eligible"])

    def test_failed_trial_keeps_status_without_fabricating_metrics(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            suite.write_new(root / 'provenance.json', {'manifest': {'repetitions': 1,
                'series': 'local', 'applications': {'bareline': {}},
                'scenarios': [{'name': 'cold_launch', 'comparable_metrics': []}]},
                'missing_scenarios': ['warm_launch']})
            suite.write_new(root / 'cold_launch-0-bareline.json', {'scenario': 'cold_launch', 'pair': 0,
                'position': 0, 'application': 'bareline', 'status': 'timeout', 'metrics': None,
                'exit_code': None, 'error': 'deadline'})
            suite.report(root, root / 'report.out')
            output = suite.read_json(root / 'report.out')
            self.assertEqual(output['schema_version'], 2)
            self.assertEqual(output['coverage'][0]['status'], 'failed')
            self.assertEqual(output['failures'][0]['status'], 'timeout')
            self.assertEqual(output['observations'], [])
            self.assertIn('one or more trials failed or were not run', output['ineligibility_reasons'])

    def test_inert_paired_run_retains_raw_trials_and_valid_source(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest_path = root / 'manifest.json'
            suite.write_new(manifest_path, inert_manifest(root))
            run = suite.run(manifest_path, root / 'runs')
            report_path = root / 'report.json'
            suite.report(run, report_path)
            report = suite.read_json(report_path)
            self.assertTrue(suite.report_source_valid(report))
            self.assertEqual(report['rows'][0]['status'], 'measured')
            self.assertEqual(report['rows'][0]['paired_samples'], 1)
            self.assertTrue(all(item['status'] == 'measured' for item in report['coverage']))
            raw = suite.read_json(run / 'save-0-bareline.json')
            self.assertEqual(raw['status'], 'ok')
            self.assertIn('"event": "measurement"', raw['stdout'])

    def test_source_mutation_retains_measurements_but_blocks_report_and_regression(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest_path = root / 'manifest.json'
            suite.write_new(manifest_path, inert_manifest(root, mutate=True))
            run = suite.run(manifest_path, root / 'runs')
            report_path = root / 'report.json'
            suite.report(run, report_path)
            report = suite.read_json(report_path)
            self.assertFalse(suite.report_source_valid(report))
            self.assertEqual(report['rows'][0]['status'], 'unqualified')
            self.assertTrue(all(item['status'] == 'measured' for item in report['coverage']))
            provenance = report['provenance']
            self.assertEqual(native_nightly.qualification_exit_code(
                provenance['source_before'], provenance['source_after'], report['coverage']), 1)
            with self.assertRaisesRegex(ValueError, 'candidate source identity'):
                suite.regress(report_path, [], root / 'regression.json')

    def test_invalid_marker_never_becomes_zero_measurement(self):
        for value in ({}, {"event": "measurement", "metrics": {"time": -1}}, {"event": "measurement", "metrics": {"time": True}}):
            with self.assertRaises(ValueError):
                suite.measurement(json.dumps(value))

    def test_named_size_cannot_be_reported_for_a_tiny_fixture(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'small.txt'
            path.write_bytes(b'PERF_NEEDLE')
            with self.assertRaises(ValueError):
                suite.validate_fixture_size(path, 'open_5gb')
            self.assertEqual(suite.validate_fixture_size(path, 'result_jump'), 11)

    def test_equal_names_are_not_implicitly_comparable(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            suite.write_new(root / 'provenance.json', {'manifest': {'repetitions': 1, 'scenarios': [{'name': 'scroll'}]}})
            for position, application in enumerate(('bareline', 'notepadpp')):
                suite.write_new(root / f'scroll-0-{application}.json', {'scenario': 'scroll', 'pair': 0,
                    'position': position, 'application': application, 'status': 'ok', 'metrics': {'operation_us': 10}})
            suite.report(root, root / 'report.out')
            output = suite.read_json(root / 'report.out')
            self.assertEqual(output['rows'], [])
            self.assertEqual(len(output['observations']), 2)

    def test_rolling_baseline_requires_exact_previous_seven(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest = {'series': 'hosted', 'configuration': 'fixed', 'machine_id': 'hosted'}
            identity = {'available': True, 'head': 'fixture', 'working_tree_dirty': False,
                        'source_manifest_sha256': 'a' * 64}
            provenance = {'manifest': manifest, 'source_before': identity, 'source_after': identity,
                          'source_changed_during_run': False}
            row = {'scenario': 'launch', 'metric': 'first_frame_us', 'sample_count': 3,
                   'status': 'measured', 'bareline': {'p50': 100}}
            for index in range(7):
                suite.write_new(root / f'{index}.json', {'provenance': provenance, 'rows': [row]})
            suite.write_new(root / 'candidate.json', {'provenance': provenance,
                'rows': [{**row, 'bareline': {'p50': 120}}]})
            baselines = [str(root / f'{index}.json') for index in range(7)]
            with self.assertRaises(ValueError):
                suite.regress(root / 'candidate.json', baselines[:6], root / 'bad.out')
            suite.regress(root / 'candidate.json', baselines, root / 'report.out')
            output = suite.read_json(root / 'report.out')
            self.assertEqual(len(output['findings']), 1)
            self.assertFalse(output['ci_failure_from_numbers'])
            suite.write_new(root / 'unqualified.json', {'provenance': provenance,
                'rows': [{**row, 'status': 'unqualified', 'bareline': {'p50': 120}}]})
            suite.regress(root / 'unqualified.json', baselines, root / 'unqualified.out')
            self.assertEqual(suite.read_json(root / 'unqualified.out')['findings'], [])


if __name__ == "__main__":
    unittest.main()
