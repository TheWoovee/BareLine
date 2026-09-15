"""Contained Windows sharing-contract evidence; generated scratch files only."""
import ctypes
from ctypes import wintypes
from pathlib import Path
import uuid

root = Path(__file__).resolve().parent
scratch = root / ('directory-sharing-probe-' + uuid.uuid4().hex)
scratch.mkdir()
kernel = ctypes.WinDLL('kernel32', use_last_error=True)
kernel.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, ctypes.c_void_p, wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
kernel.CreateFileW.restype = wintypes.HANDLE
kernel.CloseHandle.argtypes = [wintypes.HANDLE]
kernel.CloseHandle.restype = wintypes.BOOL
# DELETE | FILE_READ_ATTRIBUTES; FILE_SHARE_READ only; OPEN_EXISTING;
# FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT.
handle = kernel.CreateFileW(str(scratch), 0x00010080, 1, None, 3, 0x02200000, None)
if handle == ctypes.c_void_p(-1).value:
    scratch.rmdir()
    raise ctypes.WinError(ctypes.get_last_error())
try:
    child = scratch / 'late-child.txt'
    child.write_bytes(b'new source generation')
    assert child.read_bytes() == b'new source generation'
    print('CONFIRMED: a pinned directory denying write/delete sharing still permits a new child file.')
finally:
    kernel.CloseHandle(handle)
    if (scratch / 'late-child.txt').exists():
        (scratch / 'late-child.txt').unlink()
    scratch.rmdir()
