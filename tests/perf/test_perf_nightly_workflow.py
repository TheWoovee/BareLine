# SPDX-License-Identifier: MPL-2.0
"""Contract checks for .github/workflows/perf-nightly.yml (QA-10, SEC-12).

Text based on purpose: CI's Python has no YAML parser, and these are the lines a
reviewer must see. The workflow itself is validated by actionlint in ci.yml.
"""
from pathlib import Path
import re
import unittest

WORKFLOW = Path(__file__).resolve().parents[2] / '.github' / 'workflows' / 'perf-nightly.yml'
PERF_JOBS = ('measure', 'native-measure', 'compare-notepadpp', 'report-regressions')


def jobs(text):
    body = text.split('\njobs:\n', 1)[1]
    blocks = re.split(r'\n(?=  [A-Za-z0-9_-]+:\n)', '\n' + body)
    return {block.strip().split(':', 1)[0]: block for block in blocks if block.strip()}


def triggers(text):
    return text.split('\npermissions:', 1)[0]


def job_if(block):
    match = re.search(r'\n    if: (>-\n(?:      .*\n)+|.*\n)', block)
    return match.group(1) if match else ''


class NightlyWorkflowContract(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.text = WORKFLOW.read_text(encoding='utf-8')
        cls.jobs = jobs(cls.text)

    def test_never_triggered_by_pull_requests(self):
        self.assertNotIn('pull_request', triggers(self.text))
        self.assertIn('schedule:', triggers(self.text))
        self.assertIn('workflow_dispatch:', triggers(self.text))

    def test_self_hosted_jobs_are_guarded_to_this_repository_default_branch(self):
        self_hosted = [name for name, block in self.jobs.items() if 'self-hosted' in block]
        self.assertEqual(set(self_hosted), {'native-measure', 'compare-notepadpp'})
        for name in self_hosted:
            block = self.jobs[name]
            with self.subTest(job=name):
                condition = job_if(block)
                self.assertIn("github.repository == 'TheWoovee/BareLine'", condition)
                self.assertIn("github.ref == 'refs/heads/master'", condition)
                self.assertIn("(github.event_name == 'schedule' || github.event_name == 'workflow_dispatch')", condition)
                self.assertRegex(block, r'\n    timeout-minutes: \d+\n')
                self.assertRegex(block, r'\n    permissions:\n      contents: read\n(?!      \S)')
                self.assertNotIn('write', block.split('\n    permissions:\n', 1)[1].split('\n    ', 1)[0])
                self.assertIn('persist-credentials: false', block)
                self.assertNotIn('pull_request', block)

    def test_issue_writing_job_is_guarded_the_same_way(self):
        condition = job_if(self.jobs['report-regressions'])
        self.assertIn("github.repository == 'TheWoovee/BareLine'", condition)
        self.assertIn("github.ref == 'refs/heads/master'", condition)

    def test_perf_jobs_do_not_hide_failures(self):
        for name in PERF_JOBS:
            with self.subTest(job=name):
                self.assertNotIn('continue-on-error', self.jobs[name])

    def test_hosted_series_and_reporting_are_enabled_by_default(self):
        self.assertIn("if: vars.BARELINE_PERF_ENABLED != 'false'", self.jobs['measure'])
        self.assertNotIn("BARELINE_PERF_ISSUES_ENABLED == 'true'", self.text)

    def test_short_history_warns_and_stale_history_alerts(self):
        block = self.jobs['report-regressions']
        self.assertIn('tests/perf/perf_history.py', block)
        self.assertIn('--max-age-days', block)
        self.assertIn("steps.history.outputs.stale == 'true'", block)
        self.assertIn('gh issue create', block)
        self.assertIn("steps.history.outputs.ready == 'true'", block)
        # The old step compared only with exactly seven runs and skipped silently otherwise.
        self.assertNotIn('$previous.Count -eq 7', block)

    def test_tables_are_published_as_job_summaries(self):
        for name in PERF_JOBS:
            with self.subTest(job=name):
                self.assertIn('GITHUB_STEP_SUMMARY', self.jobs[name])

    def test_hosted_summary_carries_prebuild_source_identity(self):
        block = self.jobs['measure']
        self.assertLess(block.index('perf_suite.py source-identity'), block.index('cargo build'))
        self.assertIn('--source-before', block)

    def test_comparison_uses_the_pinned_notepadpp_installer(self):
        block = self.jobs['compare-notepadpp']
        self.assertIn('Install-NotepadPlusPlus.ps1', block)
        self.assertIn('Invoke-Bench.ps1', block)
        self.assertIn('Runs = 10', block)


if __name__ == '__main__':
    unittest.main()
