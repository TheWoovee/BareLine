"""Probe the migration handle contract using generated scratch files only."""
import ctypes
from ctypes import wintypes
from pathlib import Path
import struct
import uuid

root = Path(__file__).resolve().parent

kernel = ctypes.WinDLL("kernel32", use_last_error=True)
kernel.CreateFileW.argtypes = [
    wintypes.LPCWSTR,
    wintypes.DWORD,
    wintypes.DWORD,
    ctypes.c_void_p,
    wintypes.DWORD,
    wintypes.DWORD,
    wintypes.HANDLE,
]
kernel.CreateFileW.restype = wintypes.HANDLE
kernel.SetFileInformationByHandle.argtypes = [
    wintypes.HANDLE,
    ctypes.c_int,
    ctypes.c_void_p,
    wintypes.DWORD,
]
kernel.SetFileInformationByHandle.restype = wintypes.BOOL
kernel.CloseHandle.argtypes = [wintypes.HANDLE]

DELETE_READ_ATTRIBUTES = 0x00010080
SHARE_READ = 0x1
SHARE_DELETE = 0x4
OPEN_EXISTING = 3
NO_FOLLOW_DIRECTORY = 0x02200000
INVALID = ctypes.c_void_p(-1).value


def open_lease(path: Path, access: int, sharing: int) -> int:
    handle = kernel.CreateFileW(
        str(path),
        access,
        sharing,
        None,
        OPEN_EXISTING,
        NO_FOLLOW_DIRECTORY if path.is_dir() else 0x00200000,
        None,
    )
    if handle == INVALID:
        raise ctypes.WinError(ctypes.get_last_error())
    return handle


def probe(label: str, child_sharing: int) -> None:
    scratch = root / ("directory-rename-child-lease-" + uuid.uuid4().hex)
    source = scratch / "stage"
    target = scratch / "published"
    source.mkdir(parents=True)
    child = source / "nested.bin"
    child.write_bytes(b"verified bytes")
    parent_handle = open_lease(source, DELETE_READ_ATTRIBUTES, SHARE_READ)
    child_handle = open_lease(child, 0x80, child_sharing)
    try:
        encoded = str(target).encode("utf-16-le")
        # FILE_RENAME_INFO on 64-bit Windows: BOOLEAN at 0, HANDLE at 8,
        # FileNameLength at 16, and the UTF-16 name at 20.
        info = bytearray(20 + len(encoded) + 2)
        struct.pack_into("<Q", info, 8, 0)
        struct.pack_into("<I", info, 16, len(encoded))
        info[20 : 20 + len(encoded)] = encoded
        storage = (ctypes.c_ubyte * len(info)).from_buffer(info)
        ok = kernel.SetFileInformationByHandle(parent_handle, 3, storage, len(info))
        error = ctypes.get_last_error()
        print(label + "=" + ("succeeded" if ok else f"failed winerror={error}"))
    finally:
        kernel.CloseHandle(child_handle)
        kernel.CloseHandle(parent_handle)
        published_child = target / "nested.bin"
        if published_child.exists():
            published_child.unlink()
        elif child.exists():
            child.unlink()
        if target.exists():
            target.rmdir()
        elif source.exists():
            source.rmdir()
        scratch.rmdir()


probe("parent_rename_with_child_share_read", SHARE_READ)
probe("parent_rename_with_child_share_read_delete", SHARE_READ | SHARE_DELETE)
