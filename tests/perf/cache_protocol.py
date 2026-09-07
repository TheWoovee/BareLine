# SPDX-License-Identifier: MPL-2.0
"""Explicit cache preparation contract; no implied cold state from copied files."""
import json
from pathlib import Path
from perf_suite import capture, digest, read_json


def prepare(plan_path, plan_sha256, application, fixture, root):
    if not plan_path or digest(plan_path) != plan_sha256.lower():
        raise ValueError('cold launch requires a pinned cache preparation plan')
    plan = read_json(plan_path)
    argv = plan['argv']
    if not argv or any(not isinstance(arg, str) for arg in argv):
        raise ValueError('cache preparer requires an argument array')
    executable = Path(argv[0]).resolve()
    if digest(executable) != plan['sha256'].lower():
        raise ValueError('cache preparer executable pin mismatch')
    for pinned in plan.get('pinned_files', []):
        if digest(pinned['path']) != pinned['sha256'].lower():
            raise ValueError('cache preparer support pin mismatch')
    timeout = plan['timeout_seconds']
    if isinstance(timeout, bool) or not isinstance(timeout, (int, float)) or not 1 <= timeout <= 120:
        raise ValueError('cache preparer deadline must be 1..120 seconds')
    argv = [str(executable)] + [arg.replace('{application}', str(application))
            .replace('{fixture}', str(fixture or '')).replace('{output}', str(root)) for arg in argv[1:]]
    result = capture(argv, str(root), timeout)
    if result['status'] != 'ok':
        raise RuntimeError('cache preparation failed: ' + result['status'])
    receipts = []
    for line in result['stdout'].splitlines():
        try:
            value = json.loads(line)
        except ValueError:
            continue
        if isinstance(value, dict) and value.get('event') == 'cache_prepared':
            receipts.append(value)
    if len(receipts) != 1 or receipts[0].get('state') != 'cold' or not receipts[0].get('method'):
        raise ValueError('cache preparer did not acknowledge a cold-state protocol')
    receipt = receipts[0]
    evidence = Path(receipt['evidence_path']).resolve()
    if not evidence.is_relative_to(Path(root).resolve()) or digest(evidence) != receipt['evidence_sha256'].lower():
        raise ValueError('cache preparation evidence must be a pinned file inside this trial root')
    # Evidence is emitted into retained raw stdout before the temporary root is
    # removed; bounded JSON receipts cannot reference a disappearing-only artifact.
    if evidence.stat().st_size > 65536:
        raise ValueError('cache evidence exceeds 64 KiB')
    receipt['evidence_text'] = evidence.read_text(encoding='utf-8')
    receipt['plan_sha256'] = plan_sha256
    return receipt
