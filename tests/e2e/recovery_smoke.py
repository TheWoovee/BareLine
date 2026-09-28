# SPDX-License-Identifier: MPL-2.0
"""Verify production process-crash recovery using an isolated scratch profile.

This is not save-boundary fault injection, power-loss testing, or VM qualification.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import sys
import time

import evidence_json
import lab_fixture

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / 'perf'))


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def run(args):
    if os.name != 'nt':
        raise ValueError('Native recovery smoke requires Windows')
    executable = lab_fixture.regular(args.executable.absolute()).resolve(strict=True)
    probe = lab_fixture.regular(args.probe.absolute()).resolve(strict=True)
    if digest(executable) != args.sha256 or digest(probe) != args.probe_sha256:
        raise ValueError('Pinned editor or recovery probe hash differs')
    if (executable.parent / 'bareline.portable').exists():
        raise ValueError('Copy the editor outside its portable package before this profile test')
    output = lab_fixture.regular(args.output.absolute()).resolve()
    output.mkdir(parents=True, exist_ok=False)
    request = dict(schema_version=1, executable=str(executable), binary_sha256=args.sha256,
                   recovery_probe=str(probe), probe_sha256=args.probe_sha256,
                   scratch=str(output), dpi=args.dpi)
    sources = [HERE / name for name in ('recovery_smoke.py', 'native_recovery_smoke.ps1',
               'native_driver.ps1', 'native_session.ps1', 'native_regex_transform.ps1', 'native_lab.ps1')]
    source_hashes = {str(path): digest(path) for path in sources}
    request['driver_sources'] = source_hashes
    request_path = output / 'request.json'
    request_path.write_text(json.dumps(request, indent=2), encoding='utf-8')
    powershell = Path(os.environ['SystemRoot']) / 'System32/WindowsPowerShell/v1.0/powershell.exe'
    from windows_process_metrics import OwnedProcessTree
    child = None
    error = None
    code = None
    started = time.monotonic()
    try:
        child = OwnedProcessTree([str(powershell), '-NoProfile', '-NonInteractive', '-File',
                                  str(HERE / 'native_recovery_smoke.ps1'), '-RequestPath', str(request_path)], output)
        while (code := child.poll_exit_code()) is None:
            if time.monotonic() - started > 180:
                raise ValueError('Recovery smoke deadline exceeded')
            if (output / 'STOP').exists():
                raise ValueError('Explicit stop requested')
            time.sleep(0.05)
        if code != 0:
            raise ValueError(f'Recovery driver exited with code {code}')
    except (OSError, ValueError) as caught:
        error = str(caught)
    finally:
        if child:
            child.close()
    result_path = output / 'driver-result.json'
    result = evidence_json.read_json(result_path) if result_path.exists() else {}
    unchanged = digest(executable) == args.sha256 and digest(probe) == args.probe_sha256
    sources_unchanged = all(digest(path) == source_hashes[str(path)] for path in sources)
    passed = (error is None and unchanged and sources_unchanged and result.get('status') == 'PASS'
              and result.get('clean_exit') is True and result.get('original_pid') != result.get('recovery_pid')
              and result.get('durable_documents') == 2 and result.get('restored_documents') == 2)
    report = dict(schema_version=1, status='PASS' if passed else 'FAIL', error=error,
                  elapsed_seconds=time.monotonic()-started, driver_exit_code=code,
                  binaries_unchanged=unchanged, driver_sources_unchanged=sources_unchanged,
                  request=request, driver_result=result,
                  scope='Owned process crash after inspected durable checkpoints; saved and Untitled byte round trips')
    (output / 'result.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    print(json.dumps(dict(status=report['status'], result=str(output / 'result.json'), error=error)))
    return 0 if passed else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--executable', type=Path, required=True)
    parser.add_argument('--sha256', required=True)
    parser.add_argument('--probe', type=Path, required=True)
    parser.add_argument('--probe-sha256', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--dpi', type=int, default=100)
    try:
        return run(parser.parse_args())
    except (OSError, ValueError) as error:
        print(str(error), file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
