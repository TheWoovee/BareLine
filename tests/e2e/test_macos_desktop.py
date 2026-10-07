# SPDX-License-Identifier: MPL-2.0
"""Pure parts of the macOS journey helper; runs on every system."""
import unittest

import macos_desktop


class MacDesktopTests(unittest.TestCase):
    def test_neutral_chords_become_key_codes_and_flags(self):
        self.assertEqual(macos_desktop.chord("Primary+Shift+P"), (35, 0x100000 | 0x20000))
        self.assertEqual(macos_desktop.chord("Alt+Shift+Down"), (125, 0x80000 | 0x20000))
        self.assertEqual(macos_desktop.chord("Return"), (36, 0))
        self.assertEqual(macos_desktop.chord("Primary+End"), (119, 0x100000))
        # While the shell reads Ctrl from the Control key, Primary is Control.
        self.assertEqual(macos_desktop.chord("Primary+Shift+P", "control"), (35, 0x40000 | 0x20000))
        self.assertEqual(macos_desktop.chord("Primary+S", "command"), (1, 0x100000))
        for bad in ("Hyper+S", "Primary+Insert", "F13", "Control+F13"):
            with self.subTest(chord=bad), self.assertRaises(ValueError):
                macos_desktop.chord(bad)

    def test_text_chunks_never_split_a_surrogate_pair(self):
        text = "a" * 15 + "\U0001F389" + "b"
        chunks = macos_desktop.utf16_chunks(text)
        self.assertEqual([len(chunk) for chunk in chunks], [17, 1])
        self.assertEqual(chunks[0][15:], [0xD83C, 0xDF89])
        joined = b"".join(unit.to_bytes(2, "little") for chunk in chunks for unit in chunk)
        self.assertEqual(joined.decode("utf-16-le"), text)
        self.assertEqual(macos_desktop.utf16_chunks(""), [])


if __name__ == "__main__":
    unittest.main()
