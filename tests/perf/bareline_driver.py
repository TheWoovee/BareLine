# SPDX-License-Identifier: MPL-2.0
"""Opt-in native workload adapter; no application or filesystem work at import."""
import argparse
import ctypes as C
from ctypes import wintypes as W
import hashlib
import json
import os
from pathlib import Path
import tempfile
import time

from perf_suite import measurement, read_json
from notepadpp_driver import require_hash, require_x64_pe, file_version
from windows_process_metrics import OwnedProcessTree, MemorySampler

WORKLOADS = {
    'open_10mb': 'open', 'open_100mb': 'open', 'open_1gb': 'open', 'open_5gb': 'open',
    'long_line': 'open', 'scroll': 'scroll', 'edit_to_paint': 'edit',
    'literal_search': 'literal-search', 'regex_search': 'regex-search',
    'result_jump': 'result-jump', 'save_as': 'save-as',
    'tabs_100': 'tabs-100', 'tabs_500': 'tabs-500',
}


def run(args):
    if os.name != 'nt' or C.sizeof(C.c_void_p) != 8:
        raise OSError('adapter requires 64-bit Windows/Python')
    executable = Path(args.application).resolve()
    require_hash(executable, args.sha256)
    require_x64_pe(executable)
    if file_version(executable) != args.version:
        raise ValueError('application file version does not match pin')
    if not 1 <= args.timeout <= 120:
        raise ValueError('native workload deadline must be 1..120 seconds')
    with Path(args.config).open('rb') as config_source:
        settings = config_source.read(1024 * 1024 + 1)
    if len(settings) > 1024 * 1024 or hashlib.sha256(settings).hexdigest() != args.config_sha256.lower():
        raise ValueError('settings size/hash mismatch')
    if not args.scenario.startswith('tabs_') and not args.fixture:
        raise ValueError('scenario requires an owned fixture copy')
    with tempfile.TemporaryDirectory(prefix='bareline-native-benchmark-') as temporary:
        root = Path(temporary)
        (root / '.bareline-perf').touch(exist_ok=False)
        (root / 'settings.toml').write_bytes(settings)
        fixture = None
        copied_bytes = 0
        if args.fixture:
            source_path = Path(args.fixture).resolve()
            if not source_path.is_file():
                raise ValueError('fixture must be a regular file')
            fixture = root / ('fixture' + source_path.suffix)
            digest = hashlib.sha256()
            with source_path.open('rb') as source, fixture.open('xb') as target:
                for block in iter(lambda: source.read(65536), b''):
                    digest.update(block); target.write(block); copied_bytes += len(block)
            if digest.hexdigest() != args.fixture_sha256.lower():
                raise ValueError('fixture hash mismatch')
        argv = [str(executable), '--perf-workload', WORKLOADS[args.scenario],
                '--perf-root', str(root), '--' + args.renderer]
        if fixture:
            argv += ['--perf-input', str(fixture)]
        with OwnedProcessTree(argv, executable.parent) as tree:
            sampler = MemorySampler(tree).start()
            try:
                deadline = time.monotonic() + args.timeout
                while tree.alive():
                    if time.monotonic() >= deadline:
                        raise TimeoutError('native workload did not finish before deadline')
                    time.sleep(.01)
                get_exit = tree.api.kernel.GetExitCodeProcess
                get_exit.argtypes = [W.HANDLE, C.POINTER(W.DWORD)]
                get_exit.restype = W.BOOL
                exit_code = W.DWORD()
                tree.api.require(get_exit(tree.process, C.byref(exit_code)))
                if exit_code.value != 0:
                    raise RuntimeError(f'native workload exited with {exit_code.value}')
                receipt = root / 'performance-result.json'
                event = read_json(receipt)
                metrics = measurement(json.dumps(event))
                sampler.stop()
                metrics.update(sampler.metrics())
                metrics['owned_fixture_disk_bytes'] = copied_bytes
                print(json.dumps({'event': 'measurement', 'metrics': metrics, 'provenance': {
                    'application_sha256': args.sha256, 'version': args.version,
                    'settings_sha256': args.config_sha256, 'renderer_requested': args.renderer,
                    'cache_state_managed_by_driver': False, 'no_extensions': True,
                    'memory_peak_kind': 'maximum complete sampled live Job total',
                    'timing_scope': 'native driver/present receipts; not Scintilla roundtrip',
                }}))
            finally:
                sampler.stop()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--application', required=True)
    parser.add_argument('--sha256', required=True)
    parser.add_argument('--version', required=True)
    parser.add_argument('--config', required=True)
    parser.add_argument('--config-sha256', required=True)
    parser.add_argument('--scenario', choices=tuple(WORKLOADS), required=True)
    parser.add_argument('--fixture')
    parser.add_argument('--fixture-sha256', default='')
    parser.add_argument('--renderer', choices=('hardware', 'software'), default='hardware')
    parser.add_argument('--timeout', type=float, default=120)
    try:
        run(parser.parse_args())
    except (OSError, ValueError, RuntimeError, TimeoutError) as error:
        import sys
        print(json.dumps({'event': 'driver_error', 'error': str(error)}), file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
