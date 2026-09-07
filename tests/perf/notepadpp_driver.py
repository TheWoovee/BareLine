# SPDX-License-Identifier: MPL-2.0
"""Opt-in pinned Notepad++ benchmark adapter. Never run during a locked-source gate.

Scintilla message timing is labelled roundtrip, not frame presentation. Unsupported
signals (syntax-ready/cancel acknowledgement) produce no fabricated metrics.
"""
import argparse
import ctypes as C
from ctypes import wintypes as W
import hashlib
import json
import math
import os
from pathlib import Path
import shutil
import struct
import sys
import tempfile
import time
import xml.etree.ElementTree as ET
from windows_process_metrics import OwnedProcessTree, MemorySampler, WinApi
from cache_protocol import prepare as prepare_cache

SCENARIOS = ('cold_launch', 'warm_launch', 'empty_idle', 'open_10mb', 'open_100mb',
             'open_1gb', 'open_5gb', 'long_line', 'scroll', 'edit_to_paint',
             'literal_search', 'regex_search', 'result_jump', 'save')

def require_hash(path, expected):
    if len(expected) != 64 or any(c not in '0123456789abcdef' for c in expected.lower()):
        raise ValueError('a SHA256 pin is required')
    digest = hashlib.sha256()
    with Path(path).open('rb') as source:
        for chunk in iter(lambda: source.read(65536), b''):
            digest.update(chunk)
    if digest.hexdigest() != expected.lower():
        raise ValueError(f'SHA256 mismatch: {path}')

def require_x64_pe(path):
    # Validate the image itself, not merely the bitness of this Python process.
    with Path(path).open('rb') as source:
        header = source.read(64)
        if len(header) != 64 or header[:2] != b'MZ':
            raise ValueError('application is not a PE executable')
        offset = struct.unpack_from('<I', header, 60)[0]
        if not 64 <= offset <= 16 * 1024 * 1024:
            raise ValueError('invalid PE header offset')
        source.seek(offset); image = source.read(26)
        if len(image) != 26 or image[:4] != b'PE\x00\x00':
            raise ValueError('invalid PE signature')
        machine = struct.unpack_from('<H', image, 4)[0]
        optional_size, characteristics = struct.unpack_from('<HH', image, 20)
        magic = struct.unpack_from('<H', image, 24)[0]
        if machine != 0x8664 or optional_size < 2 or magic != 0x20B or not characteristics & 2 or characteristics & 0x2000:
            raise ValueError('adapter requires an x64 PE32+ executable, not a DLL')

def file_version(path):
    api = C.WinDLL('version', use_last_error=True)
    api.GetFileVersionInfoSizeW.argtypes = [W.LPCWSTR, C.POINTER(W.DWORD)]
    api.GetFileVersionInfoSizeW.restype = W.DWORD
    api.GetFileVersionInfoW.argtypes = [W.LPCWSTR, W.DWORD, W.DWORD, C.c_void_p]
    api.GetFileVersionInfoW.restype = W.BOOL
    api.VerQueryValueW.argtypes = [C.c_void_p, W.LPCWSTR, C.POINTER(C.c_void_p), C.POINTER(W.UINT)]
    api.VerQueryValueW.restype = W.BOOL
    size = api.GetFileVersionInfoSizeW(str(path), None)
    if not 0 < size <= 1024 * 1024:
        raise ValueError('missing/oversized executable version resource')
    data = C.create_string_buffer(size)
    WinApi.require(api.GetFileVersionInfoW(str(path), 0, size, data))
    pointer = C.c_void_p(); length = W.UINT()
    WinApi.require(api.VerQueryValueW(data, '\\', C.byref(pointer), C.byref(length)))
    if length.value < 52:
        raise ValueError('invalid fixed version resource')
    fields = struct.unpack('<13I', C.string_at(pointer, 52))
    if fields[0] != 0xFEEF04BD:
        raise ValueError('invalid fixed version signature')
    return '.'.join(str(value) for value in (fields[2] >> 16, fields[2] & 65535,
                                           fields[3] >> 16, fields[3] & 65535))

class WindowDriver:
    def __init__(self, tree, deadline):
        self.tree = tree; self.deadline = deadline
        self.user = C.WinDLL('user32', use_last_error=True)
        self.callback = C.WINFUNCTYPE(W.BOOL, W.HWND, W.LPARAM)
        self.user.EnumWindows.argtypes = [self.callback, W.LPARAM]; self.user.EnumWindows.restype = W.BOOL
        self.user.EnumChildWindows.argtypes = [W.HWND, self.callback, W.LPARAM]; self.user.EnumChildWindows.restype = W.BOOL
        self.user.GetWindowThreadProcessId.argtypes = [W.HWND, C.POINTER(W.DWORD)]; self.user.GetWindowThreadProcessId.restype = W.DWORD
        self.user.GetClassNameW.argtypes = [W.HWND, W.LPWSTR, C.c_int]; self.user.GetClassNameW.restype = C.c_int
        self.user.IsWindowVisible.argtypes = [W.HWND]; self.user.IsWindowVisible.restype = W.BOOL
        self.user.SendMessageTimeoutW.argtypes = [W.HWND, W.UINT, W.WPARAM, W.LPARAM, W.UINT, W.UINT, C.POINTER(C.c_size_t)]
        self.user.SendMessageTimeoutW.restype = W.LPARAM
        self.main = None; self.editor = None; self.remote_uncertain = False
    def remaining_ms(self):
        remaining = self.deadline - time.monotonic()
        if remaining <= 0 or not self.tree.alive():
            raise TimeoutError('benchmark deadline or process exit')
        return max(1, min(5000, int(remaining * 1000)))
    def owner(self, window):
        pid = W.DWORD(); self.user.GetWindowThreadProcessId(window, C.byref(pid)); return pid.value
    def class_name(self, window):
        value = C.create_unicode_buffer(256); self.user.GetClassNameW(window, value, len(value)); return value.value
    def message(self, window, message, wparam=0, lparam=0):
        if self.owner(window) != self.tree.pid:
            raise RuntimeError('window ownership changed')
        result = C.c_size_t()
        if not self.user.SendMessageTimeoutW(window, message, wparam, lparam, 0x2 | 0x20,
                                            self.remaining_ms(), C.byref(result)):
            # Receiver may still be using a remote message pointer. Kill the owned
            # Job before any remote storage can be released.
            self.remote_uncertain = True
            self.tree.api.terminate_job(self.tree.job, 124)
            self.tree.api.wait(self.tree.process, 5000)
            raise TimeoutError('owned Notepad++ message did not acknowledge')
        return C.c_ssize_t(result.value).value
    def ready(self, expected_length):
        while True:
            self.remaining_ms(); windows = []
            @self.callback
            def collect(window, _):
                if self.owner(window) == self.tree.pid and self.class_name(window) == 'Notepad++' and self.user.IsWindowVisible(window):
                    windows.append(window)
                return True
            self.user.EnumWindows(collect, 0)
            for main in windows:
                editors = []
                @self.callback
                def children(window, _):
                    if self.owner(window) == self.tree.pid and self.class_name(window) == 'Scintilla' and self.user.IsWindowVisible(window):
                        editors.append(window)
                    return True
                self.user.EnumChildWindows(main, children, 0)
                for editor in editors:
                    if self.message(editor, 2006) == expected_length:
                        self.main = main; self.editor = editor; self.message(main, 0)
                        return
            time.sleep(0.01)
    def scintilla(self, message, wparam=0, lparam=0):
        return self.message(self.editor, message, wparam, lparam)
    def with_bytes(self, value, action):
        if not 0 < len(value) <= 65536:
            raise ValueError('remote command payload must be 1..65536 bytes')
        kernel = self.tree.api.kernel
        kernel.VirtualAllocEx.argtypes = [W.HANDLE, C.c_void_p, C.c_size_t, W.DWORD, W.DWORD]; kernel.VirtualAllocEx.restype = C.c_void_p
        kernel.VirtualFreeEx.argtypes = [W.HANDLE, C.c_void_p, C.c_size_t, W.DWORD]; kernel.VirtualFreeEx.restype = W.BOOL
        kernel.WriteProcessMemory.argtypes = [W.HANDLE, C.c_void_p, C.c_void_p, C.c_size_t, C.POINTER(C.c_size_t)]; kernel.WriteProcessMemory.restype = W.BOOL
        pointer = WinApi.require(kernel.VirtualAllocEx(self.tree.process, None, len(value), 0x3000, 4))
        try:
            buffer = C.create_string_buffer(value); written = C.c_size_t()
            WinApi.require(kernel.WriteProcessMemory(self.tree.process, pointer, buffer, len(value), C.byref(written)))
            if written.value != len(value):
                raise OSError('partial remote command write')
            return action(pointer)
        finally:
            # On an unacknowledged send the receiver may retain this pointer.
            # Never free it early, even if termination/wait failed; process exit
            # reclaims the allocation. The enclosing owned Job is still closed.
            if not self.remote_uncertain:
                kernel.VirtualFreeEx(self.tree.process, pointer, 0, 0x8000)


def timed(action):
    start = time.perf_counter_ns(); value = action(); return (time.perf_counter_ns() - start) / 1000, value

def run(args):
    if os.name != 'nt' or C.sizeof(C.c_void_p) != 8:
        raise OSError('adapter requires 64-bit Windows/Python and a pinned x64 Notepad++')
    executable = Path(args.application).resolve(); require_hash(executable, args.sha256)
    require_x64_pe(executable)
    actual_version = file_version(executable)
    if actual_version != args.version:
        raise ValueError(f'version mismatch: {actual_version}')
    config = Path(args.config).resolve(); require_hash(config, args.config_sha256)
    if config.stat().st_size > 1024 * 1024:
        raise ValueError('configuration exceeds 1 MiB')
    config_bytes = config.read_bytes()
    if hashlib.sha256(config_bytes).hexdigest() != args.config_sha256.lower():
        raise ValueError('configuration changed during pin validation')
    parsed = ET.fromstring(config_bytes)
    updates = parsed.findall(".//GUIConfig[@name='noUpdate']")
    if len(updates) != 1 or (updates[0].text or '').strip() != 'yes' or updates[0].get('autoUpdateMode', '0') != '0':
        raise ValueError('pinned configuration must explicitly disable automatic updates')
    backups = parsed.findall(".//GUIConfig[@name='Backup']")
    if len(backups) != 1:
        raise ValueError('pinned configuration must explicitly disable backups')
    backup = backups[0]
    if backup.get('action') != '0' or backup.get('useCustumDir') != 'no' or backup.get('dir', '').strip() or backup.get('isSnapshotMode') != 'no':
        raise ValueError('backup/snapshot mode and external backup directory must be disabled')
    if not 1 <= args.timeout <= 3600 or not 1 <= args.samples <= 1000 or not 0 <= args.idle_seconds <= 60:
        raise ValueError('invalid benchmark bounds')
    if args.fixture and (args.expected_text_bytes is None or args.expected_text_bytes < 0):
        raise ValueError('fixture needs expected Scintilla UTF-8-view byte length')
    if args.fixture and args.scenario in ('cold_launch', 'warm_launch', 'empty_idle'):
        raise ValueError('launch/empty-idle scenarios do not accept a fixture')
    if args.scenario not in ('cold_launch', 'warm_launch', 'empty_idle') and not args.fixture:
        raise ValueError('scenario needs a pinned fixture')
    with tempfile.TemporaryDirectory(prefix='bareline-npp-benchmark-') as temporary:
        root = Path(temporary); settings = root / 'settings'; settings.mkdir()
        (settings / 'config.xml').write_bytes(config_bytes)
        fixture = None; fixture_bytes = 0
        if args.fixture:
            fixture = root / 'fixture.txt'; digest = hashlib.sha256()
            with Path(args.fixture).open('rb') as source, fixture.open('xb') as target:
                for chunk in iter(lambda: source.read(65536), b''):
                    digest.update(chunk); target.write(chunk); fixture_bytes += len(chunk)
            if digest.hexdigest() != args.fixture_sha256.lower():
                raise ValueError('fixture SHA256 mismatch')
        argv = [str(executable), '-multiInst', '-noPlugin', '-nosession', f'-settingsDir={settings}']
        if fixture:
            argv.append(str(fixture))
        if args.scenario == 'warm_launch':
            with OwnedProcessTree(argv, executable.parent) as warm:
                WindowDriver(warm, time.monotonic() + args.timeout).ready(0)
        cache_receipt = None
        if args.scenario == 'cold_launch':
            cache_receipt = prepare_cache(args.cache_plan, args.cache_plan_sha256, executable, fixture, root)
        started = time.perf_counter_ns(); deadline = time.monotonic() + args.timeout
        with OwnedProcessTree(argv, executable.parent) as tree:
            sampler = MemorySampler(tree).start()
            try:
                driver = WindowDriver(tree, deadline); expected = args.expected_text_bytes if fixture else 0
                driver.ready(expected)
                metrics = {'launch_to_responsive_us': (time.perf_counter_ns() - started) / 1000}
                if fixture:
                    metrics['open_to_responsive_us'] = metrics['launch_to_responsive_us']
                if args.scenario == 'empty_idle':
                    until = time.monotonic() + args.idle_seconds
                    while time.monotonic() < until:
                        driver.remaining_ms(); time.sleep(0.02)
                    point = tree.sample()
                    if point.missing_processes:
                        raise RuntimeError('incomplete idle memory sample')
                    metrics.update(private_bytes_idle_total=point.private_bytes_total,
                                   working_set_bytes_idle_total=point.working_set_bytes_total)
                elif args.scenario == 'scroll':
                    durations = []
                    for index in range(args.samples):
                        before = driver.scintilla(2152)
                        # Alternate the same bounded viewport displacement to avoid
                        # benchmarking repeated no-ops at the document bottom.
                        lines = 10 if index % 2 == 0 else -10
                        elapsed, _ = timed(lambda: driver.scintilla(2168, 0, lines))
                        after = driver.scintilla(2152)
                        if after == before or (after - before) * lines <= 0:
                            raise RuntimeError('scroll did not move the visible viewport in the requested direction')
                        durations.append(elapsed)
                    metrics.update(scroll_roundtrip_p50_us=sorted(durations)[(len(durations)-1)//2],
                                   scroll_roundtrip_p95_us=sorted(durations)[math.ceil(len(durations)*.95)-1],
                                   scroll_roundtrip_max_us=max(durations))
                elif args.scenario == 'edit_to_paint':
                    elapsed, _ = timed(lambda: driver.with_bytes(b'x', lambda pointer: driver.scintilla(2001, 1, pointer)))
                    if driver.scintilla(2006) != expected + 1:
                        raise RuntimeError('edit was not applied')
                    metrics['edit_roundtrip_us'] = elapsed
                    driver.scintilla(2176)
                    if driver.scintilla(2006) != expected:
                        raise RuntimeError('edit undo failed')
                elif args.scenario in ('literal_search', 'regex_search'):
                    value = args.pattern.encode('utf-8')
                    driver.scintilla(2382, 0)  # Clear prior Scintilla status.
                    driver.scintilla(2190, 0); driver.scintilla(2192, expected)
                    driver.scintilla(2198, 0x00200000 if args.scenario == 'regex_search' else 0)
                    elapsed, found = timed(lambda: driver.with_bytes(value, lambda pointer: driver.scintilla(2197, len(value), pointer)))
                    status = driver.scintilla(2383)
                    if found < -1 or status != 0:
                        raise RuntimeError(f'Scintilla search failed: result={found}, status={status}')
                    metrics.update(scintilla_search_roundtrip_us=elapsed, search_found=int(found >= 0), search_requested_bytes=expected)
                elif args.scenario == 'result_jump':
                    if not 0 <= args.position <= expected:
                        raise ValueError('jump position outside text view')
                    if expected == 0:
                        raise ValueError('result jump requires a nonempty fixture')
                    # Put the caret elsewhere before timing so a no-op cannot pass.
                    driver.scintilla(2025, expected if args.position == 0 else 0)
                    before = driver.scintilla(2008)
                    if before == args.position:
                        raise RuntimeError('could not establish distinct jump origin')
                    elapsed, _ = timed(lambda: driver.scintilla(2025, args.position))
                    if driver.scintilla(2008) != args.position:
                        raise RuntimeError('result jump did not reach the requested text position')
                    metrics['result_jump_roundtrip_us'] = elapsed
                elif args.scenario == 'save':
                    driver.with_bytes(b'x', lambda pointer: driver.scintilla(2001, 1, pointer))
                    elapsed, _ = timed(lambda: driver.message(driver.main, 0x0111, 41006))
                    if driver.scintilla(2159) != 0:
                        raise RuntimeError('save did not clear modified state')
                    metrics['save_command_roundtrip_us'] = elapsed
                sampler.stop(); metrics.update(sampler.metrics())
                metrics.update(owned_fixture_disk_bytes=fixture_bytes, downloaded_bytes=0)
                print(json.dumps({'event':'measurement','metrics':metrics,
                                  'provenance':{'application_sha256':args.sha256,'version':actual_version,
                                  'plugins_disabled':True,'config_sha256':args.config_sha256,
                                  'memory_peak_kind':'maximum complete sampled live Job total',
                                  'paint_completion_measured':False,'cache_preparation':cache_receipt,
                                  'warmup_launches':int(args.scenario == 'warm_launch'),
                                  'download_counter_scope':'adapter only',
                                  'search_scope':'Scintilla target-search primitive, not Find dialog cancellation'}}))
            finally:
                sampler.stop()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--application', required=True); parser.add_argument('--sha256', required=True)
    parser.add_argument('--version', required=True, help='exact four-part PE file version')
    parser.add_argument('--config', required=True); parser.add_argument('--config-sha256', required=True)
    parser.add_argument('--scenario', choices=SCENARIOS, required=True)
    parser.add_argument('--fixture'); parser.add_argument('--fixture-sha256', default='')
    parser.add_argument('--expected-text-bytes', type=int); parser.add_argument('--timeout', type=float, default=60)
    parser.add_argument('--samples', type=int, default=100); parser.add_argument('--idle-seconds', type=float, default=2)
    parser.add_argument('--pattern', default='bareline-benchmark-absent-token'); parser.add_argument('--position', type=int, default=0)
    parser.add_argument('--cache-plan'); parser.add_argument('--cache-plan-sha256', default='')
    try:
        run(parser.parse_args())
    except (OSError, ValueError, RuntimeError, TimeoutError, ET.ParseError) as error:
        print(json.dumps({'event':'driver_error','error':str(error)}), file=sys.stderr); return 1
    return 0

if __name__ == '__main__':
    raise SystemExit(main())
