# SPDX-License-Identifier: MPL-2.0
"""Negative controls for the owned recovery-process supervisor."""
from contextlib import redirect_stdout
import io
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import recovery_smoke as smoke


class RecoverySmokeTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.editor = self.root / 'editor.exe'
        self.probe = self.root / 'probe.exe'
        self.editor.write_bytes(b'synthetic editor; never launched')
        self.probe.write_bytes(b'synthetic probe; never launched')
        self.args = SimpleNamespace(executable=self.editor, sha256=smoke.digest(self.editor),
                                    probe=self.probe, probe_sha256=smoke.digest(self.probe),
                                    output=self.root / 'run', dpi=100)
        self.environment = patch.object(smoke, 'os', SimpleNamespace(name='nt', environ={'SystemRoot': str(self.root)}))
        self.environment.start()
        self.addCleanup(self.environment.stop)

    def fixture(self, changes=None, exit_code=0, mutate_binary=False):
        child = Mock()
        child.poll_exit_code.return_value = exit_code
        if mutate_binary:
            child.close.side_effect = lambda: self.editor.write_bytes(b'changed during cleanup')
        result = dict(status='PASS', clean_exit=True, original_pid=101, recovery_pid=202,
                      durable_documents=2, restored_documents=2)
        result.update(changes or {})

        def launch(argv, directory):
            self.assertEqual(directory, self.args.output)
            self.assertEqual(Path(argv[-1]), directory / 'request.json')
            (directory / 'driver-result.json').write_text(json.dumps(result), encoding='utf-8')
            return child

        with patch('windows_process_metrics.OwnedProcessTree', side_effect=launch), redirect_stdout(io.StringIO()):
            status = smoke.run(self.args)
        child.close.assert_called_once()
        return status, json.loads((self.args.output / 'result.json').read_text())

    def test_both_round_trips_and_distinct_recovery_process_are_required(self):
        for change in (dict(durable_documents=1), dict(restored_documents=1), dict(recovery_pid=101), dict(clean_exit=False)):
            self.args.output = self.root / ('run-' + str(len(list(self.root.iterdir()))))
            with self.subTest(change=change):
                self.assertEqual(self.fixture(change)[0], 1)

    def test_complete_observed_recovery_and_clean_exit_pass(self):
        status, result = self.fixture()
        self.assertEqual((status, result['status']), (0, 'PASS'))
        self.assertTrue(result['binaries_unchanged'])
        self.assertTrue(result['driver_sources_unchanged'])

    def test_nonzero_exit_cannot_be_hidden_by_a_pass_record(self):
        status, result = self.fixture(exit_code=7)
        self.assertEqual(status, 1)
        self.assertIn('code 7', result['error'])

    def test_binary_change_during_cleanup_invalidates_a_pass(self):
        status, result = self.fixture(mutate_binary=True)
        self.assertEqual(status, 1)
        self.assertFalse(result['binaries_unchanged'])

    def test_wrong_pin_and_portable_profile_are_refused_before_launch(self):
        with patch('windows_process_metrics.OwnedProcessTree') as launch:
            self.args.sha256 = '0' * 64
            with self.assertRaisesRegex(ValueError, 'hash differs'):
                smoke.run(self.args)
            self.args.sha256 = smoke.digest(self.editor)
            (self.editor.parent / 'bareline.portable').write_bytes(b'')
            with self.assertRaisesRegex(ValueError, 'portable'):
                smoke.run(self.args)
            launch.assert_not_called()
        self.assertFalse(self.args.output.exists())

    def test_existing_results_are_never_replaced(self):
        self.args.output.mkdir()
        marker = self.args.output / 'result.json'
        marker.write_bytes(b'prior result')
        with patch('windows_process_metrics.OwnedProcessTree') as launch:
            with self.assertRaises(FileExistsError):
                smoke.run(self.args)
            launch.assert_not_called()
        self.assertEqual(marker.read_bytes(), b'prior result')


if __name__ == '__main__':
    unittest.main()
