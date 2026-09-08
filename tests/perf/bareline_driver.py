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

from perf_suite import measurement, read_json, validate_fixture_size
from notepadpp_driver import require_hash, require_x64_pe, file_version
from windows_process_metrics import OwnedProcessTree, MemorySampler
from cache_protocol import prepare as prepare_cache

WORKLOADS = {
    'cold_launch': 'launch', 'warm_launch': 'launch', 'empty_idle': 'idle',
    'open_10mb': 'open', 'open_100mb': 'open', 'open_1gb': 'open', 'open_5gb': 'open',
    'long_line': 'open', 'scroll': 'scroll', 'edit_to_paint': 'edit',
    'literal_search': 'literal-search', 'regex_search': 'regex-search',
    'result_jump': 'result-jump', 'save': 'save', 'save_as': 'save-as',
    'syntax_viewport': 'syntax', 'search_cancel': 'search-cancel',
    'workspace_scan': 'workspace-scan', 'tail_append': 'tail-append',
    'extensions_memory': 'extensions-memory',
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
    if args.fixture and args.scenario in ('cold_launch', 'warm_launch', 'empty_idle', 'tabs_100', 'tabs_500'):
        raise ValueError('launch/empty-idle/populated-tab scenarios do not accept an input fixture')
    with Path(args.config).open('rb') as config_source:
        settings = config_source.read(1024 * 1024 + 1)
    if len(settings) > 1024 * 1024 or hashlib.sha256(settings).hexdigest() != args.config_sha256.lower():
        raise ValueError('settings size/hash mismatch')
    if not args.scenario.startswith('tabs_') and args.scenario not in ('cold_launch', 'warm_launch', 'empty_idle') and not args.fixture:
        raise ValueError('scenario requires an owned fixture copy')
    with tempfile.TemporaryDirectory(prefix='bareline-native-benchmark-') as temporary:
        root = Path(temporary)
        (root / '.bareline-perf').touch(exist_ok=False)
        (root / 'settings.toml').write_bytes(settings)
        extension_bytes = 0
        if args.scenario == 'extensions_memory':
            if not args.extension_inventory or not args.extension_command:
                raise ValueError('extension scenario requires a pinned installed fixture inventory and owner/command')
            require_hash(args.extension_inventory, args.extension_inventory_sha256)
            inventory = read_json(args.extension_inventory)
            source_root = Path(args.extension_inventory).resolve().parent
            files = inventory['files']
            if not isinstance(files, list) or not 1 <= len(files) <= 4096:
                raise ValueError('extension fixture requires 1..4096 pinned files')
            for record in files:
                relative = Path(record['path'])
                source = (source_root / relative).resolve()
                if relative.is_absolute() or '..' in relative.parts or not source.is_relative_to(source_root) or not source.is_file():
                    raise ValueError('extension fixture path escapes inventory root')
                target = root / 'extensions' / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                digest = hashlib.sha256()
                with source.open('rb') as original, target.open('xb') as copied:
                    for block in iter(lambda: original.read(65536), b''):
                        extension_bytes += len(block)
                        if extension_bytes > 512 * 1024 * 1024:
                            raise ValueError('extension fixture exceeds 512 MiB copy quota')
                        digest.update(block); copied.write(block)
                if digest.hexdigest() != record['sha256'].lower():
                    raise ValueError('extension fixture file hash mismatch')
        fixture = None
        copied_bytes = 0
        if args.fixture:
            source_path = Path(args.fixture).resolve()
            validate_fixture_size(source_path, args.scenario)
            fixture_root = root / 'workspace'
            fixture_root.mkdir()
            fixture = fixture_root / ('fixture' + source_path.suffix)
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
        if args.scenario == 'extensions_memory':
            argv += ['--perf-extension-command', args.extension_command]
        cache_receipt = None
        if args.scenario == 'warm_launch':
            # Complete one unmeasured identical launch before the measured launch.
            with OwnedProcessTree(argv, executable.parent) as warm:
                deadline = time.monotonic() + args.timeout
                while warm.alive():
                    if time.monotonic() >= deadline:
                        raise TimeoutError('warm-up launch did not finish')
                    time.sleep(.01)
                measurement(json.dumps(read_json(root / 'performance-result.json')))
            (root / 'performance-result.json').rename(root / 'warmup-result.json')
        if args.scenario == 'cold_launch':
            cache_receipt = prepare_cache(args.cache_plan, args.cache_plan_sha256, executable, fixture, root)
        argv += ['--perf-origin-ns', str(time.perf_counter_ns())]
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
                if args.scenario == 'empty_idle':
                    idle = next((point for point in reversed(sampler.points) if point.process_count > 0 and not point.missing_processes), None)
                    if idle is None or idle.elapsed_ns < 9_000_000_000:
                        raise RuntimeError('no complete sample near the end of the idle window')
                    metrics.update(private_bytes_idle_total=idle.private_bytes_total,
                                   working_set_bytes_idle_total=idle.working_set_bytes_total)
                if args.scenario == 'extensions_memory' and metrics['process_count'] < 2:
                    raise RuntimeError('sampler did not observe the extension child; no process-tree comparison emitted')
                metrics['owned_fixture_disk_bytes'] = copied_bytes
                metrics['extension_fixture_disk_bytes'] = extension_bytes
                print(json.dumps({'event': 'measurement', 'metrics': metrics, 'provenance': {
                    'application_sha256': args.sha256, 'version': args.version,
                    'settings_sha256': args.config_sha256, 'renderer_requested': args.renderer,
                    'cache_preparation': cache_receipt, 'warmup_launches': int(args.scenario == 'warm_launch'),
                    'no_extensions': args.scenario != 'extensions_memory',
                    'extension_inventory_sha256': args.extension_inventory_sha256 or None,
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
    parser.add_argument('--cache-plan')
    parser.add_argument('--cache-plan-sha256', default='')
    parser.add_argument('--extension-inventory')
    parser.add_argument('--extension-inventory-sha256', default='')
    parser.add_argument('--extension-command')
    try:
        run(parser.parse_args())
    except (OSError, ValueError, RuntimeError, TimeoutError) as error:
        import sys
        print(json.dumps({'event': 'driver_error', 'error': str(error)}), file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
