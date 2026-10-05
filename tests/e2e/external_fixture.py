# SPDX-License-Identifier: MPL-2.0
"""Owned, bounded external-command fixture; never launches a shell."""
import json
import os
from pathlib import Path
import subprocess
import sys
import time


def main():
    if sys.argv[1:] == ['--child']:
        time.sleep(60)
        return 0
    root, target = (Path(value).resolve(strict=True) for value in sys.argv[1:3])
    if not root.is_dir() or target.parent != root or target.name != 'macro-source.txt':
        raise ValueError('Only the generated macro scratch target is allowed')
    receipt = root / 'external-receipt.json'
    if receipt.exists(): raise ValueError('Fixture receipt already exists')
    child = subprocess.Popen([sys.executable, str(Path(__file__).resolve()), '--child'],
                             cwd=root, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                             creationflags=0x08000000 if os.name == 'nt' else 0)
    try:
        stage = receipt.with_suffix('.tmp')
        with stage.open('x', encoding='utf-8') as stream:
            json.dump(dict(pid=os.getpid(), child_pid=child.pid, argv=sys.argv[3:],
                           cwd=str(Path.cwd()), target=str(target)), stream)
            stream.flush()
            os.fsync(stream.fileno())
        stage.replace(receipt)
        print(str(target) + ':2:1', flush=True)
        time.sleep(60)
    finally:
        if child.poll() is None: child.terminate()
        child.wait(timeout=5)
    return 0


if __name__ == '__main__': raise SystemExit(main())
