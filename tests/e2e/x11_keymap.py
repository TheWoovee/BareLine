#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0
"""Bind characters to spare keycodes of an X display before the journeys type them.

xdotool types a character outside the keymap by rebinding a scratch keycode
just before each key event and restoring it afterwards; a client that reads
the event after the next rebinding sees another character (U+0301 arrives as
the following CJK character, or not at all). The journey harness
(xtask/src/journey/linux.rs) runs this once per new character on its private
Xvfb display, so every character it types already has a keycode of its own
and no keycode changes while the editor reads keys.

Usage: x11_keymap.py <characters>
Prints a JSON object mapping each character to its keycode (0 when no spare
keycode was left). Uses only libX11 through ctypes.
"""
import ctypes
import json
import sys

NO_SYMBOL = 0


def keysym(character):
    code = ord(character)
    # Latin-1 keysyms equal their code points; others are 0x01000000 + code point.
    return code if 0x20 <= code <= 0x7E or 0xA0 <= code <= 0xFF else 0x01000000 + code


def main():
    x11 = ctypes.cdll.LoadLibrary("libX11.so.6")
    x11.XOpenDisplay.restype = ctypes.c_void_p
    x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x11.XDisplayKeycodes.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_int)]
    x11.XGetKeyboardMapping.restype = ctypes.POINTER(ctypes.c_ulong)
    x11.XGetKeyboardMapping.argtypes = [ctypes.c_void_p, ctypes.c_ubyte, ctypes.c_int, ctypes.POINTER(ctypes.c_int)]
    x11.XKeysymToKeycode.restype = ctypes.c_ubyte
    x11.XKeysymToKeycode.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
    x11.XChangeKeyboardMapping.argtypes = [
        ctypes.c_void_p,
        ctypes.c_int,
        ctypes.c_int,
        ctypes.POINTER(ctypes.c_ulong),
        ctypes.c_int,
    ]
    x11.XFree.argtypes = [ctypes.c_void_p]
    x11.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]
    x11.XCloseDisplay.argtypes = [ctypes.c_void_p]

    display = x11.XOpenDisplay(None)
    if not display:
        sys.exit("cannot open the X display")
    low, high = ctypes.c_int(), ctypes.c_int()
    x11.XDisplayKeycodes(display, ctypes.byref(low), ctypes.byref(high))
    count = high.value - low.value + 1
    per = ctypes.c_int()
    table = x11.XGetKeyboardMapping(display, low.value, count, ctypes.byref(per))
    spare = [
        low.value + index
        for index in range(count)
        if all(table[index * per.value + column] == NO_SYMBOL for column in range(per.value))
    ]
    x11.XFree(table)
    bound = {}
    for character in dict.fromkeys(sys.argv[1] if len(sys.argv) > 1 else ""):
        sym = keysym(character)
        code = x11.XKeysymToKeycode(display, sym)
        if code == 0 and spare:
            # Highest spare first: xdotool takes its scratch keycode from the same pool.
            code = spare.pop()
            symbols = (ctypes.c_ulong * 2)(sym, sym)
            x11.XChangeKeyboardMapping(display, code, 2, symbols, 1)
        bound[character] = code
    x11.XSync(display, 0)
    x11.XCloseDisplay(display)
    print(json.dumps(bound, ensure_ascii=False))


if __name__ == "__main__":
    main()
