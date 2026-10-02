# SPDX-License-Identifier: MPL-2.0
"""Nightly baseline history, staleness and hosted-summary regressions; never launches an editor."""
from datetime import datetime, timedelta, timezone
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

import perf_history
import perf_suite as suite

NOW = datetime(2026, 9, 30, 4, 0, tzinfo=timezone.utc)
IDENTITY = {'available': True, 'head': 'fixture', 'working_tree_dirty': False, 'source_manifest_sha256': 'a' * 64}


def report(p50=100, machine='hosted', valid=True):
    source = IDENTITY if valid else {'available': False, 'reason': 'fixture'}
    return {'provenance': {'manifest': {'series': 'hosted', 'configuration': 'fixed', 'machine_id': machine},
                           'source_before': source, 'source_after': source, 'source_changed_during_run': False},
            'rows': [{'scenario': 'launch', 'metric': 'first_frame_us', 'sample_count': 3,
                      'status': 'measured', 'bareline': {'p50': p50}}]}


def history(count, newest_age=timedelta(days=1), **kwargs):
    return [{'run_id': 1000 - index, 'created_at': (NOW - newest_age - timedelta(days=index)).isoformat(),
             'path': f'history/{1000 - index}/hosted-report.json', 'report': report(**kwargs)}
            for index in range(count)]


class BaselineHistory(unittest.TestCase):
    def test_short_history_warns_and_records_instead_of_silently_skipping(self):
        result = perf_history.evaluate(report(), history(3), NOW, max_age_days=3)
        self.assertFalse(result['ready'])
        self.assertFalse(result['stale'])
        self.assertEqual(len(result['selected']), 3)
        self.assertTrue(any(a.startswith('::warning') and '3 of 7' in a for a in result['annotations']))
        self.assertIn('this run is recorded', perf_history.render(result, 'hosted'))

    def test_seven_comparable_baselines_are_selected_newest_first(self):
        runs = history(9)
        runs[1]['report'] = None
        runs[2]['report'] = report(machine='other-machine')
        result = perf_history.evaluate(report(), runs, NOW, max_age_days=3)
        self.assertTrue(result['ready'])
        self.assertEqual([item['run_id'] for item in result['selected']], [1000, 997, 996, 995, 994, 993, 992])
        reasons = {item['run_id']: item['reason'] for item in result['excluded']}
        self.assertIn('absent', reasons[999])
        self.assertIn('cannot mix machines', reasons[998])
        self.assertEqual(result['annotations'], [])

    def test_old_newest_baseline_is_stale(self):
        result = perf_history.evaluate(report(), history(7, newest_age=timedelta(days=5)), NOW, max_age_days=3)
        self.assertTrue(result['stale'])
        self.assertTrue(result['ready'])
        self.assertTrue(any(a.startswith('::error title=Stale performance baseline') for a in result['annotations']))

    def test_missing_candidate_is_an_error_not_a_skip(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'runs.json').write_text(json.dumps([{'databaseId': 5, 'createdAt': NOW.isoformat()}]), encoding='utf-8')
            code = perf_history.main(['--candidate', '', '--runs', str(root / 'runs.json'), '--history-dir',
                                      str(root / 'history'), '--report-name', 'hosted-report.json', '--series', 'hosted',
                                      '--now', NOW.isoformat(), '--output', str(root / 'selection.json'),
                                      '--github-output', str(root / 'outputs.txt')])
            self.assertEqual(code, 1)
            self.assertEqual((root / 'outputs.txt').read_text(encoding='utf-8'), 'ready=false\nstale=false\n')

    def test_cli_reads_downloaded_artifacts_and_writes_outputs(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            runs = []
            for index in range(7):
                run_id = 200 + index
                target = root / 'history' / str(run_id) / f'hosted-windows-perf-{run_id}'
                target.mkdir(parents=True)
                suite.write_new(target / 'hosted-report.json', report())
                runs.append({'databaseId': run_id, 'createdAt': (NOW - timedelta(hours=10 + index)).isoformat()})
            (root / 'runs.json').write_text(json.dumps(runs), encoding='utf-8')
            suite.write_new(root / 'candidate.json', report(p50=150))
            summary = root / 'summary.md'
            code = perf_history.main(['--candidate', str(root / 'candidate.json'), '--runs', str(root / 'runs.json'),
                                      '--history-dir', str(root / 'history'), '--report-name', 'hosted-report.json',
                                      '--series', 'hosted', '--now', NOW.isoformat(), '--output',
                                      str(root / 'selection.json'), '--summary', str(summary),
                                      '--github-output', str(root / 'outputs.txt')])
            self.assertEqual(code, 0)
            self.assertEqual((root / 'outputs.txt').read_text(encoding='utf-8'), 'ready=true\nstale=false\n')
            selection = suite.read_json(root / 'selection.json')
            suite.regress(root / 'candidate.json', [item['path'] for item in selection['selected']],
                          root / 'regressions.json')
            self.assertEqual(len(suite.read_json(root / 'regressions.json')['findings']), 1)
            self.assertIn('Comparable baselines: 7 of 7', summary.read_text(encoding='utf-8'))


class HostedSummary(unittest.TestCase):
    def repository(self, root):
        root.mkdir()
        (root / 'tracked.txt').write_text('source', encoding='utf-8')
        (root / '.gitignore').write_text('results/\n', encoding='utf-8')
        for arguments in (('init',), ('add', '.'), ('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                                                     'commit', '-m', 'fixture')):
            subprocess.run(['git', *arguments], cwd=root, check=True, capture_output=True)

    def raw_launch(self, directory, frame_us):
        directory.mkdir(parents=True, exist_ok=True)
        suite.write_new(directory / f'launch-{frame_us}.json', {
            'kind': 'visible_launch_idle', 'profile': 'release', 'os_build': '26100',
            'samples': [{'requested_software': True, 'frame': {'microseconds': frame_us},
                         'idle': {'private_bytes': 24 * 1024 * 1024}} for _ in range(3)]})

    def test_hosted_series_can_reach_the_rolling_regression(self):
        # Before QA-10 the hosted summary had no source identity or row status, so
        # regress() refused every candidate and no hosted regression was ever reported.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / 'source'
            self.repository(source)
            before = suite.t09_source_identity(source)
            reports = []
            for index, frame_us in enumerate([100000] * 7 + [150000]):
                results = source / 'results' / str(index)
                self.raw_launch(results, frame_us)
                destination = results / 'hosted-report.json'
                suite.summarize_legacy(results, destination, before, source_root=source)
                reports.append(destination)
            candidate = suite.read_json(reports[-1])
            self.assertTrue(suite.report_source_valid(candidate))
            self.assertTrue(all(row['status'] == 'measured' for row in candidate['rows']))
            suite.regress(reports[-1], [str(path) for path in reports[:7]], root / "regressions.json")
            findings = suite.read_json(root / 'regressions.json')['findings']
            self.assertEqual([(f['scenario'], f['metric']) for f in findings],
                             [('visible_launch_idle-software', 'first_frame_us')])

    def test_hosted_summary_without_prebuild_identity_stays_unqualified(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / 'source'
            self.repository(source)
            results = source / 'results'
            self.raw_launch(results, 100000)
            suite.summarize_legacy(results, results / 'hosted-report.json', None, source_root=source)
            summary = suite.read_json(results / 'hosted-report.json')
            self.assertFalse(suite.report_source_valid(summary))
            self.assertTrue(all(row['status'] == 'unqualified' for row in summary['rows']))
            with self.assertRaisesRegex(ValueError, 'candidate source identity'):
                suite.regress(results / 'hosted-report.json', [], Path(directory) / 'out.json')


if __name__ == '__main__':
    unittest.main()
