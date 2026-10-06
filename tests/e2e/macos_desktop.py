# SPDX-License-Identifier: MPL-2.0
"""macOS desktop helper of the port journeys (xtask/src/journey/macos.rs).

  windows [PID]       JSON list of on-screen, normal-layer windows (of PID when given):
                      id, pid, x, y, width, height (points) and title (null without
                      screen-recording permission)
  trusted             "true" when this process may post synthetic input
                      (System Settings > Privacy & Security > Accessibility)
  capture-allowed     "true" when this process may capture other windows and read
                      their titles (System Settings > Privacy & Security > Screen Recording)
  key PID CHORD       post one chord to PID, neutral names joined by "+":
                      Primary, Shift, Alt (Option), letters, digits, Return,
                      Escape, Tab, Space, Home, End, Up, Down, Left, Right, PageDown, F1..F12
  text PID TEXT       post Unicode text to PID

Primary is Command, or Control when BARELINE_QA_MAC_PRIMARY=control (xtask
sets it while the shell still reads its keymap's Ctrl from the Control key).

Quartz is reached through ctypes (CoreGraphics, CoreFoundation and
ApplicationServices by full framework path), so no PyObjC is needed. Events go
to the process with CGEventPostToPid and need no keyboard focus.
"""
import json
import os
import sys

FLAGS = {"Shift": 0x20000, "Control": 0x40000, "Alt": 0x80000, "Command": 0x100000}
PRIMARY = {"command": "Command", "control": "Control"}
KEY_CODES = {
    "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9, "b": 11, "q": 12,
    "w": 13, "e": 14, "r": 15, "y": 16, "t": 17, "1": 18, "2": 19, "3": 20, "4": 21, "6": 22, "5": 23,
    "9": 25, "7": 26, "8": 28, "0": 29, "o": 31, "u": 32, "i": 34, "p": 35, "l": 37, "j": 38, "k": 40,
    "n": 45, "m": 46, "Return": 36, "Tab": 48, "Space": 49, "Escape": 53, "Home": 115, "PageDown": 121,
    "End": 119, "Left": 123, "Right": 124, "Down": 125, "Up": 126, "F1": 122, "F2": 120, "F3": 99,
    "F4": 118, "F5": 96, "F6": 97, "F7": 98, "F8": 100, "F9": 101, "F10": 109, "F11": 103, "F12": 111,
}


def chord(text, primary="command"):
    """(key code, modifier flags) of one neutral chord; Primary is `primary`."""
    *modifiers, key = text.split("+")
    flags = 0
    for modifier in modifiers:
        if modifier == "Primary":
            modifier = PRIMARY[primary]
        if modifier not in FLAGS:
            raise ValueError(f"unknown modifier {modifier!r} in {text!r}")
        flags |= FLAGS[modifier]
    code = KEY_CODES.get(key) if len(key) > 1 else KEY_CODES.get(key.lower())
    if code is None:
        raise ValueError(f"unknown key {key!r} in {text!r}")
    return code, flags


def utf16_chunks(text, size=16):
    """UTF-16 code units in chunks that never split a surrogate pair."""
    units = list(text.encode("utf-16-le"))
    units = [units[i] | units[i + 1] << 8 for i in range(0, len(units), 2)]
    chunks, current = [], []
    for unit in units:
        if len(current) >= size and not 0xDC00 <= unit <= 0xDFFF:
            chunks.append(current)
            current = []
        current.append(unit)
    if current:
        chunks.append(current)
    return chunks


class Quartz:
    ON_SCREEN_ONLY, EXCLUDE_DESKTOP, SINT64, UTF8 = 1 << 0, 1 << 4, 4, 0x08000100

    def __init__(self):
        import ctypes
        self.ctypes = ctypes
        base = "/System/Library/Frameworks/"
        self.cg = ctypes.CDLL(base + "CoreGraphics.framework/CoreGraphics")
        self.cf = ctypes.CDLL(base + "CoreFoundation.framework/CoreFoundation")
        self.ax = ctypes.CDLL(base + "ApplicationServices.framework/ApplicationServices")
        pointer = ctypes.c_void_p
        cg, cf = self.cg, self.cf
        self.keys = {name: ctypes.c_void_p.in_dll(cg, name).value for name in (
            "kCGWindowOwnerPID", "kCGWindowLayer", "kCGWindowNumber", "kCGWindowBounds", "kCGWindowName")}
        cg.CGWindowListCopyWindowInfo.argtypes, cg.CGWindowListCopyWindowInfo.restype = [ctypes.c_uint32, ctypes.c_uint32], pointer
        cf.CFArrayGetCount.argtypes, cf.CFArrayGetCount.restype = [pointer], ctypes.c_long
        cf.CFArrayGetValueAtIndex.argtypes, cf.CFArrayGetValueAtIndex.restype = [pointer, ctypes.c_long], pointer
        cf.CFDictionaryGetValue.argtypes, cf.CFDictionaryGetValue.restype = [pointer, pointer], pointer
        cf.CFNumberGetValue.argtypes, cf.CFNumberGetValue.restype = [pointer, ctypes.c_long, pointer], ctypes.c_bool
        cf.CFStringGetCString.argtypes, cf.CFStringGetCString.restype = [pointer, ctypes.c_char_p, ctypes.c_long, ctypes.c_uint32], ctypes.c_bool
        cf.CFRelease.argtypes, cf.CFRelease.restype = [pointer], None

        class Rect(ctypes.Structure):
            _fields_ = [("x", ctypes.c_double), ("y", ctypes.c_double), ("width", ctypes.c_double), ("height", ctypes.c_double)]
        self.Rect = Rect
        cg.CGRectMakeWithDictionaryRepresentation.argtypes = [pointer, ctypes.POINTER(Rect)]
        cg.CGRectMakeWithDictionaryRepresentation.restype = ctypes.c_bool
        cg.CGEventCreateKeyboardEvent.argtypes, cg.CGEventCreateKeyboardEvent.restype = [pointer, ctypes.c_uint16, ctypes.c_bool], pointer
        cg.CGEventSetFlags.argtypes, cg.CGEventSetFlags.restype = [pointer, ctypes.c_uint64], None
        cg.CGEventKeyboardSetUnicodeString.argtypes = [pointer, ctypes.c_ulong, ctypes.POINTER(ctypes.c_uint16)]
        cg.CGEventKeyboardSetUnicodeString.restype = None
        cg.CGEventPostToPid.argtypes, cg.CGEventPostToPid.restype = [ctypes.c_int, pointer], None
        self.ax.AXIsProcessTrusted.restype = ctypes.c_bool
        cg.CGPreflightScreenCaptureAccess.restype = ctypes.c_bool

    def number(self, dictionary, key):
        value = self.cf.CFDictionaryGetValue(dictionary, self.keys[key])
        out = self.ctypes.c_int64()
        if value and self.cf.CFNumberGetValue(value, self.SINT64, self.ctypes.byref(out)):
            return out.value
        return None

    def string(self, dictionary, key):
        value = self.cf.CFDictionaryGetValue(dictionary, self.keys[key])
        buffer = self.ctypes.create_string_buffer(1024)
        if value and self.cf.CFStringGetCString(value, buffer, len(buffer), self.UTF8):
            return buffer.value.decode("utf-8", "replace")
        return None

    def windows(self, pid=None):
        array = self.cg.CGWindowListCopyWindowInfo(self.ON_SCREEN_ONLY | self.EXCLUDE_DESKTOP, 0)
        if not array:
            raise SystemExit("the window server returned no window list (no GUI session)")
        found = []
        try:
            for index in range(self.cf.CFArrayGetCount(array)):
                entry = self.cf.CFArrayGetValueAtIndex(array, index)
                owner = self.number(entry, "kCGWindowOwnerPID")
                if self.number(entry, "kCGWindowLayer") != 0 or (pid is not None and owner != pid):
                    continue
                rect = self.Rect()
                bounds = self.cf.CFDictionaryGetValue(entry, self.keys["kCGWindowBounds"])
                if not bounds or not self.cg.CGRectMakeWithDictionaryRepresentation(bounds, self.ctypes.byref(rect)):
                    continue
                found.append({"id": str(self.number(entry, "kCGWindowNumber")), "pid": owner,
                              "x": round(rect.x), "y": round(rect.y), "width": round(rect.width),
                              "height": round(rect.height), "title": self.string(entry, "kCGWindowName")})
        finally:
            self.cf.CFRelease(array)
        return found

    def post(self, pid, code, flags, down, units=None):
        event = self.cg.CGEventCreateKeyboardEvent(None, code, down)
        if not event:
            raise SystemExit("CGEventCreateKeyboardEvent failed")
        try:
            self.cg.CGEventSetFlags(event, flags)
            if units:
                buffer = (self.ctypes.c_uint16 * len(units))(*units)
                self.cg.CGEventKeyboardSetUnicodeString(event, len(units), buffer)
            self.cg.CGEventPostToPid(pid, event)
        finally:
            self.cf.CFRelease(event)


def main(argv):
    if not argv:
        raise SystemExit(__doc__)
    command, arguments = argv[0], argv[1:]
    import time
    quartz = Quartz()
    if command == "windows":
        print(json.dumps(quartz.windows(int(arguments[0]) if arguments else None)))
    elif command == "trusted":
        print("true" if quartz.ax.AXIsProcessTrusted() else "false")
    elif command == "capture-allowed":
        print("true" if quartz.cg.CGPreflightScreenCaptureAccess() else "false")
    elif command == "key":
        code, flags = chord(arguments[1], os.environ.get("BARELINE_QA_MAC_PRIMARY", "command"))
        quartz.post(int(arguments[0]), code, flags, True)
        quartz.post(int(arguments[0]), code, flags, False)
    elif command == "text":
        for units in utf16_chunks(arguments[1]):
            quartz.post(int(arguments[0]), 0, 0, True, units)
            quartz.post(int(arguments[0]), 0, 0, False, units)
            time.sleep(0.02)
    else:
        raise SystemExit(__doc__)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
