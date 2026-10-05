# SPDX-License-Identifier: MPL-2.0
"""Exercise native startup readiness without launching an editor or using input."""
import os
from pathlib import Path
import subprocess
import unittest


class NativeSessionTests(unittest.TestCase):
    @unittest.skipUnless(os.name == 'nt', 'Windows PowerShell readiness checks')
    def test_owned_first_frame_precedes_window_readiness(self):
        shell = Path(os.environ['SystemRoot']) / 'System32/WindowsPowerShell/v1.0/powershell.exe'
        result = subprocess.run([str(shell), '-NoProfile', '-NonInteractive', '-File',
                                 str(Path(__file__).with_suffix('.ps1'))],
                                capture_output=True, text=True, timeout=20)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('readiness checks passed', result.stdout)


if __name__ == '__main__':
    unittest.main()
