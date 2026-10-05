# SPDX-License-Identifier: MPL-2.0
"""Owned Windows process trees and sampled memory metrics for opt-in benchmarks.

No work occurs at import. The caller must explicitly launch a pinned executable.
Job membership, not process-name matching, defines the measured/terminated tree.
"""
import ctypes as C
from ctypes import wintypes as W
from dataclasses import dataclass
import os
from pathlib import Path
import subprocess
import threading
import time

SIZE_T = C.c_size_t
ULONG_PTR = C.c_size_t

class STARTUPINFO(C.Structure):
    _fields_ = [('cb', W.DWORD), ('lpReserved', W.LPWSTR), ('lpDesktop', W.LPWSTR),
                ('lpTitle', W.LPWSTR), ('dwX', W.DWORD), ('dwY', W.DWORD),
                ('dwXSize', W.DWORD), ('dwYSize', W.DWORD), ('dwXCountChars', W.DWORD),
                ('dwYCountChars', W.DWORD), ('dwFillAttribute', W.DWORD),
                ('dwFlags', W.DWORD), ('wShowWindow', W.WORD), ('cbReserved2', W.WORD),
                ('lpReserved2', C.c_void_p), ('hStdInput', W.HANDLE),
                ('hStdOutput', W.HANDLE), ('hStdError', W.HANDLE)]
class PROCESS_INFORMATION(C.Structure):
    _fields_ = [('hProcess', W.HANDLE), ('hThread', W.HANDLE),
                ('dwProcessId', W.DWORD), ('dwThreadId', W.DWORD)]
class BASIC_LIMIT(C.Structure):
    _fields_ = [('PerProcessUserTimeLimit', C.c_longlong), ('PerJobUserTimeLimit', C.c_longlong),
                ('LimitFlags', W.DWORD), ('MinimumWorkingSetSize', SIZE_T),
                ('MaximumWorkingSetSize', SIZE_T), ('ActiveProcessLimit', W.DWORD),
                ('Affinity', ULONG_PTR), ('PriorityClass', W.DWORD), ('SchedulingClass', W.DWORD)]
class IO_COUNTERS(C.Structure):
    _fields_ = [(name, C.c_ulonglong) for name in ('ReadOperationCount', 'WriteOperationCount',
                'OtherOperationCount', 'ReadTransferCount', 'WriteTransferCount', 'OtherTransferCount')]
class EXTENDED_LIMIT(C.Structure):
    _fields_ = [('BasicLimitInformation', BASIC_LIMIT), ('IoInfo', IO_COUNTERS),
                ('ProcessMemoryLimit', SIZE_T), ('JobMemoryLimit', SIZE_T),
                ('PeakProcessMemoryUsed', SIZE_T), ('PeakJobMemoryUsed', SIZE_T)]
class MEMORY_COUNTERS(C.Structure):
    _fields_ = [('cb', W.DWORD), ('PageFaultCount', W.DWORD), ('PeakWorkingSetSize', SIZE_T),
                ('WorkingSetSize', SIZE_T), ('QuotaPeakPagedPoolUsage', SIZE_T),
                ('QuotaPagedPoolUsage', SIZE_T), ('QuotaPeakNonPagedPoolUsage', SIZE_T),
                ('QuotaNonPagedPoolUsage', SIZE_T), ('PagefileUsage', SIZE_T),
                ('PeakPagefileUsage', SIZE_T), ('PrivateUsage', SIZE_T)]

class WinApi:
    def __init__(self):
        if os.name != 'nt':
            raise OSError('Windows process metrics require Windows')
        self.kernel = C.WinDLL('kernel32', use_last_error=True)
        self.psapi = C.WinDLL('psapi', use_last_error=True)
        def bind(dll, name, restype, args):
            fn = getattr(dll, name); fn.restype = restype; fn.argtypes = args; return fn
        self.close = bind(self.kernel, 'CloseHandle', W.BOOL, [W.HANDLE])
        self.create_job = bind(self.kernel, 'CreateJobObjectW', W.HANDLE, [C.c_void_p, W.LPCWSTR])
        self.set_job = bind(self.kernel, 'SetInformationJobObject', W.BOOL, [W.HANDLE, C.c_int, C.c_void_p, W.DWORD])
        self.query_job = bind(self.kernel, 'QueryInformationJobObject', W.BOOL, [W.HANDLE, C.c_int, C.c_void_p, W.DWORD, C.POINTER(W.DWORD)])
        self.assign = bind(self.kernel, 'AssignProcessToJobObject', W.BOOL, [W.HANDLE, W.HANDLE])
        self.terminate_job = bind(self.kernel, 'TerminateJobObject', W.BOOL, [W.HANDLE, W.UINT])
        self.terminate_process = bind(self.kernel, 'TerminateProcess', W.BOOL, [W.HANDLE, W.UINT])
        self.resume = bind(self.kernel, 'ResumeThread', W.DWORD, [W.HANDLE])
        self.wait = bind(self.kernel, 'WaitForSingleObject', W.DWORD, [W.HANDLE, W.DWORD])
        self.exit_code = bind(self.kernel, 'GetExitCodeProcess', W.BOOL, [W.HANDLE, C.POINTER(W.DWORD)])
        self.create = bind(self.kernel, 'CreateProcessW', W.BOOL, [W.LPCWSTR, W.LPWSTR, C.c_void_p, C.c_void_p, W.BOOL, W.DWORD, C.c_void_p, W.LPCWSTR, C.POINTER(STARTUPINFO), C.POINTER(PROCESS_INFORMATION)])
        self.open_process = bind(self.kernel, 'OpenProcess', W.HANDLE, [W.DWORD, W.BOOL, W.DWORD])
        self.in_job = bind(self.kernel, 'IsProcessInJob', W.BOOL, [W.HANDLE, W.HANDLE, C.POINTER(W.BOOL)])
        self.times = bind(self.kernel, 'GetProcessTimes', W.BOOL, [W.HANDLE] + [C.POINTER(W.FILETIME)] * 4)
        self.memory = bind(self.psapi, 'GetProcessMemoryInfo', W.BOOL, [W.HANDLE, C.POINTER(MEMORY_COUNTERS), W.DWORD])
    @staticmethod
    def require(value):
        if not value:
            raise C.WinError(C.get_last_error())
        return value

@dataclass(frozen=True)
class MemoryPoint:
    elapsed_ns: int
    private_bytes_total: int
    working_set_bytes_total: int
    process_count: int
    missing_processes: tuple
    identities: tuple

class OwnedProcessTree:
    """Create suspended, assign Job before execution, then resume. No breakaway."""
    def __init__(self, argv, cwd=None, environment=None):
        if not argv or not Path(argv[0]).is_absolute():
            raise ValueError('absolute executable required')
        self.api = WinApi(); self.job = None; self.process = None; self.pid = None
        self.started_ns = time.perf_counter_ns()
        self.job = self.api.require(self.api.create_job(None, None))
        info = PROCESS_INFORMATION()
        try:
            limits = EXTENDED_LIMIT(); limits.BasicLimitInformation.LimitFlags = 0x2000  # KILL_ON_JOB_CLOSE
            self.api.require(self.api.set_job(self.job, 9, C.byref(limits), C.sizeof(limits)))
            startup = STARTUPINFO(); startup.cb = C.sizeof(startup)
            command = C.create_unicode_buffer(subprocess.list2cmdline([str(a) for a in argv]))
            environment_block = None
            if environment is not None:
                if any(not isinstance(key, str) or not isinstance(value, str) or '\x00' in key + value or '=' in key for key, value in environment.items()):
                    raise ValueError('invalid explicit process environment')
                environment_block = C.create_unicode_buffer('\x00'.join(key + '=' + value for key, value in sorted(environment.items(), key=lambda pair: pair[0].upper())) + '\x00\x00')
            self.api.require(self.api.create(str(argv[0]), command, None, None, False,
                                            0x4 | 0x08000000 | (0x400 if environment is not None else 0), environment_block, str(cwd) if cwd else None,
                                            C.byref(startup), C.byref(info)))
            self.process = info.hProcess; self.pid = int(info.dwProcessId)
            self.api.require(self.api.assign(self.job, self.process))
            if self.api.resume(info.hThread) == 0xFFFFFFFF:
                raise C.WinError(C.get_last_error())
        except BaseException:
            if self.process:
                self.api.terminate_process(self.process, 125)
            self.close()
            raise
        finally:
            if info.hThread:
                self.api.close(info.hThread)
    def alive(self):
        return self.process is not None and self.api.wait(self.process, 0) == 258
    def poll_exit_code(self):
        """Return None while running, or the terminal DWORD before close()."""
        if self.process is None:
            raise OSError('Process handle is closed; terminal status unavailable')
        waited = self.api.wait(self.process, 0)
        if waited == 258:  # WAIT_TIMEOUT
            return None
        self.api.require(waited != 0xFFFFFFFF)  # WAIT_FAILED
        if waited != 0:  # Only WAIT_OBJECT_0 proves the process terminated.
            raise OSError(f'Unexpected process wait result: {waited}')
        code = W.DWORD()
        self.api.require(self.api.exit_code(self.process, C.byref(code)))
        # A signaled process can legitimately have exited with 259 (STILL_ACTIVE).
        return int(code.value)
    def process_ids(self):
        # A bounded buffer handles all admitted benchmark descendants. Exceeding
        # it is an explicit missing measurement, never silent partial totals.
        class IDS(C.Structure):
            _fields_ = [('assigned', W.DWORD), ('count', W.DWORD), ('ids', ULONG_PTR * 4096)]
        ids = IDS(); self.api.require(self.api.query_job(self.job, 3, C.byref(ids), C.sizeof(ids), None))
        if ids.count > 4096 or ids.assigned > 4096:
            raise RuntimeError('benchmark process count exceeds 4096')
        return [int(ids.ids[i]) for i in range(ids.count)]
    def sample(self):
        private = working = 0; missing = []; identities = []
        for pid in self.process_ids():
            handle = self.api.open_process(0x1000 | 0x10, False, pid)
            if not handle:
                missing.append(pid); continue
            try:
                member = W.BOOL()
                self.api.require(self.api.in_job(handle, self.job, C.byref(member)))
                if not member.value:
                    missing.append(pid); continue  # PID reused outside this Job.
                created, exited, kernel, user = (W.FILETIME() for _ in range(4))
                self.api.require(self.api.times(handle, C.byref(created), C.byref(exited), C.byref(kernel), C.byref(user)))
                counters = MEMORY_COUNTERS(); counters.cb = C.sizeof(counters)
                self.api.require(self.api.memory(handle, C.byref(counters), C.sizeof(counters)))
                private += int(counters.PrivateUsage); working += int(counters.WorkingSetSize)
                identities.append((pid, (created.dwHighDateTime << 32) | created.dwLowDateTime))
            except OSError:
                missing.append(pid)
            finally:
                self.api.close(handle)
        return MemoryPoint(time.perf_counter_ns() - self.started_ns, private, working,
                           len(identities), tuple(missing), tuple(identities))
    def close(self):
        if self.job:
            self.api.terminate_job(self.job, 0); self.api.close(self.job); self.job = None
        if self.process:
            self.api.wait(self.process, 5000); self.api.close(self.process); self.process = None
    def __enter__(self):
        return self
    def __exit__(self, *_):
        self.close()

class MemorySampler:
    """Sample aggregate live memory. Peak-of-samples is not an OS lifetime peak."""
    def __init__(self, tree, interval_seconds=0.02):
        if not 0.005 <= interval_seconds <= 1:
            raise ValueError('sampling interval must be 5..1000 ms')
        self.tree = tree; self.interval = interval_seconds; self.points = []; self.errors = []
        self.stop_event = threading.Event(); self.thread = None; self.identity_entries = 0
    def start(self):
        if self.thread is not None:
            raise RuntimeError('sampler already started')
        def worker():
            while not self.stop_event.is_set():
                try:
                    point = self.tree.sample()
                    self.identity_entries += len(point.identities) + len(point.missing_processes)
                    self.points.append(point)
                    if len(self.points) >= 10000 or self.identity_entries >= 200000:
                        self.errors.append('sample quota reached'); return
                except (OSError, RuntimeError) as error:
                    self.errors.append(str(error)); return
                self.stop_event.wait(self.interval)
        self.thread = threading.Thread(target=worker, name='benchmark-memory', daemon=True)
        self.thread.start(); return self
    def stop(self):
        self.stop_event.set()
        if self.thread is not None:
            self.thread.join(timeout=2)
            if self.thread.is_alive():
                self.errors.append('sampler did not stop')
        return self
    def metrics(self):
        complete = [p for p in self.points if not p.missing_processes]
        if self.errors or not complete or len(complete) != len(self.points):
            raise RuntimeError('incomplete process memory samples; do not publish partial totals')
        return {'private_bytes_peak_total': max(p.private_bytes_total for p in complete),
                'working_set_bytes_peak_total': max(p.working_set_bytes_total for p in complete),
                'process_count': max(p.process_count for p in complete),
                'memory_sample_count': len(complete),
                'memory_sample_interval_us': int(self.interval * 1_000_000)}
