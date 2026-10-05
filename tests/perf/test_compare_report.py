# SPDX-License-Identifier: MPL-2.0
"""Report and table regressions for the Bareline vs Notepad++ harness; never launches an editor."""
import json
from pathlib import Path
import tempfile
import unittest

import compare_report
import perf_suite


def trial(scenario, config, run, metrics, cache='warm', fixture='', status='ok', error=None):
    application = 'bareline' if config.startswith('bl-') else 'notepadpp'
    return {'id': f'{scenario}/{fixture or "-"}/{cache}/{config}/{run}', 'scenario': scenario, 'fixture': fixture,
            'cache': cache, 'config': config, 'application': application, 'run': run, 'order': 0,
            'status': status, 'metrics': metrics, 'observations': {}, 'error': error}


def raw(trials, runs=2, cache_states=('warm',), reference=False, checks=()):
    return {'schema_version': 1, 'kind': 'bareline_notepadpp_comparison',
            'protocol': {'name': 'FC-10', 'runs': runs, 'cache_states': list(cache_states), 'idle_seconds': 10,
                         'cold_cache_command': None, 'reference_machine': reference},
            'machine': {'label': 'fixture', 'cpu': 'fixture cpu', 'os': 'fixture os'},
            'applications': {'bareline': {'version': '0.1.0', 'sha256': 'a' * 64},
                             'notepadpp': {'version': '8.9.8.1', 'sha256': 'b' * 64}},
            'trials': list(trials), 'warmups': [], 'checks': list(checks)}


class ComparisonReport(unittest.TestCase):
    def write(self, directory, value):
        path = Path(directory) / 'raw.json'
        path.write_text(json.dumps(value), encoding='utf-8')
        return path

    def run_report(self, value):
        directory = tempfile.mkdtemp()
        path = self.write(directory, value)
        markdown = compare_report.comparison(path, Path(directory) / 'results.md', Path(directory) / 'summary.json')
        return markdown, perf_suite.read_json(Path(directory) / 'summary.json')

    def test_medians_ratios_and_table_for_both_applications(self):
        trials = [trial('launch', 'npp-noPlugin', 1, {'window_visible_ms': 280, 'idle_private_mb': 60.0}),
                  trial('launch', 'npp-noPlugin', 2, {'window_visible_ms': 300, 'idle_private_mb': 59.0}),
                  trial('launch', 'bl-software', 1, {'window_visible_ms': 70, 'idle_private_mb': 24.0,
                                                     'first_frame_ms': 110}),
                  trial('launch', 'bl-software', 2, {'window_visible_ms': 90, 'idle_private_mb': 23.0,
                                                     'first_frame_ms': 108})]
        markdown, summary = self.run_report(raw(trials))
        rows = {(r['config'], r['metric']): r for r in summary['rows']}
        # An even sample count uses the mean of the two middle values.
        self.assertEqual(rows[('npp-noPlugin', 'window_visible_ms')]['median'], 290)
        self.assertEqual(rows[('bl-software', 'window_visible_ms')]['median'], 80)
        ratio = {(r['config'], r['metric']): r['ratio'] for r in summary['ratios']}
        self.assertAlmostEqual(ratio[('bl-software', 'window_visible_ms')], 80 / 290)
        # Notepad++ first paint is not observable here: no ratio, and the table says why.
        self.assertNotIn(('bl-software', 'first_frame_ms'), ratio)
        self.assertIn('needs ETW', markdown)
        self.assertIn('| Metric | npp-noPlugin | bl-software | bl-software / npp-noPlugin |', markdown)
        self.assertIn('| Process start to main window visible | 290 ms (280 ms-300 ms) | 80 ms (70 ms-90 ms) | 0.28x |',
                      markdown)
        self.assertFalse(summary['claims_eligible'])

    def test_failed_trials_are_listed_and_never_become_zero(self):
        trials = [trial('replace', 'npp-default', 1, {'replace_all_ms': 1600}, fixture='log_50mb'),
                  trial('replace', 'bl-default', 1, {}, fixture='log_50mb', status='timeout',
                        error='Replace All incomplete after 180 s'),
                  trial('copy', 'bl-default', 1, {'copy_ms': 5}, fixture='text_6mb', status='failed',
                        error='clipboard holds 0 of 6262000 characters')]
        markdown, summary = self.run_report(raw(trials, runs=1))
        rows = {(r['scenario'], r['config'], r['metric']): r for r in summary['rows']}
        self.assertNotIn(('replace', 'bl-default', 'replace_all_ms'), rows)
        copy = rows[('copy', 'bl-default', 'copy_ms')]
        self.assertEqual((copy['samples'], copy['median']), (0, None))
        self.assertEqual(len(summary['failures']), 2)
        self.assertIn('Replace All incomplete after 180 s', markdown)
        # The failed configuration keeps its column, with its attempt count.
        self.assertIn('| Replace All to document updated | 1,600 ms | n/a [0/1] | n/a |', markdown)
        self.assertIn('2 trial(s) failed or timed out', summary['qualification']['reasons'])

    def test_fc10_qualification_requires_ten_runs_both_cache_states_and_reference_machine(self):
        _, summary = self.run_report(raw([trial('launch', 'npp-default', 1, {'ready_ms': 500})], runs=3))
        reasons = ' '.join(summary['qualification']['reasons'])
        self.assertFalse(summary['qualification']['fc10_qualified'])
        self.assertIn('fewer than 10', reasons)
        self.assertIn('cold', reasons)
        self.assertIn('reference machine', reasons)
        complete = [trial('launch', 'npp-default', run, {'ready_ms': 500 + run}, cache=cache)
                    for cache in ('warm', 'cold') for run in range(1, 11)]
        value = raw(complete, runs=10, cache_states=('warm', 'cold'), reference=True)
        value['protocol']['cold_cache_command'] = {'path': 'evict.ps1', 'sha256': 'c' * 64}
        _, summary = self.run_report(value)
        self.assertTrue(summary['qualification']['fc10_qualified'], summary['qualification'])

    def test_invalid_metric_values_are_rejected(self):
        for value in (-1, True, float('nan'), '12'):
            with self.subTest(value=value), self.assertRaises(ValueError):
                compare_report.aggregate(raw([trial('launch', 'npp-default', 1, {'ready_ms': value})]))

    def test_checks_are_reported(self):
        checks = [{'name': 'regex keeps CRLF', 'application': 'bareline', 'passed': False,
                   'detail': 'alpha\\nbeta\\n'}]
        markdown, summary = self.run_report(raw([], checks=checks))
        self.assertIn('| regex keeps CRLF | bareline | FAIL | alpha\\nbeta\\n |', markdown)
        self.assertEqual(summary['checks'], checks)

    def test_wrong_raw_kind_is_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            path = self.write(directory, {'schema_version': 1, 'kind': 'something_else', 'trials': []})
            with self.assertRaises(ValueError):
                compare_report.read_raw(path)


class SeriesSummaries(unittest.TestCase):
    def test_series_table_shows_unmeasured_values_as_na(self):
        report = {'provenance': {'manifest': {'series': 'hosted', 'machine_id': 'hosted', 'commit': 'abc'}},
                  'rows': [{'scenario': 'visible_launch_idle-software', 'metric': 'first_frame_us', 'sample_count': 3,
                            'bareline': {'p50': 109000, 'p95': 120000}, 'notepadpp': {'p50': None, 'p95': None},
                            'status': 'measured'}]}
        markdown = compare_report.render_series(report, 'Hosted Windows series')
        self.assertIn('| visible_launch_idle-software | first_frame_us | 3 | 109,000 | 120,000 | n/a | measured |',
                      markdown)

    def test_regressions_annotate_without_failing(self):
        result = {'findings': [{'scenario': 'launch', 'metric': 'first_frame_us', 'baseline_p50': 100,
                                'current_p50': 130, 'increase_fraction': .3, 'noise_tolerance_fraction': .1}]}
        markdown, annotations = compare_report.render_regressions(result, 'hosted')
        self.assertEqual(len(annotations), 1)
        self.assertTrue(annotations[0].startswith('::warning title=Performance regression (hosted)::'))
        self.assertIn('| launch | first_frame_us | 100 | 130 | 30.0% | 10.0% |', markdown)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'regressions.json'
            path.write_text(json.dumps(result), encoding='utf-8')
            summary = Path(directory) / 'summary.md'
            self.assertEqual(compare_report.main(['regressions', str(path), '--series', 'hosted',
                                                  '--append', str(summary)]), 0)
            written = summary.read_text(encoding='utf-8')
            self.assertIn('Rolling regressions (hosted)', written)
            self.assertNotIn('::warning', written)


if __name__ == '__main__':
    unittest.main()
