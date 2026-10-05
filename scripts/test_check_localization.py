# SPDX-License-Identifier: MPL-2.0
"""Localization resource audit (BIZ-30) on this repository and on a synthetic source tree."""
import contextlib
import io
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import check_localization as audit

REGISTRATION = '''
pub fn register(registry: &mut CommandRegistry) {
    for (id, title) in [("demo.keyed", "Keyed Command"), ("demo.new", "Brand New"), ("encoding.convert.demo", "UTF-Demo")] {
        registry.register(CommandSpec { id: CommandId(id), title, category: "Tools", shortcut: "", action: Action::Contributed(CommandId(id)) });
    }
}
#[cfg(test)]
mod tests {
    fn fixture(registry: &mut CommandRegistry) {
        registry.register(CommandSpec { id: CommandId("test.only"), title: "Test Only", category: "Tools", shortcut: "", action: Action::New });
    }
}
'''
MENUS = 'pub const TREE: &[MenuTemplate] = &[Sub("Tools", &[C("demo.keyed"), Sub("Fresh Submenu", &[C("demo.new")])])];\n'
DISCOVERY = 'pub const MENU_TAXONOMY: [&str; 1] = ["Tools"];\npub const OTHER_MENU: &str = "Other";\n'
ENGLISH = '''version = 1
locale = "en"
direction = "ltr"
[messages]
"command.demo.keyed" = "Keyed Command"
"menu.Tools" = "Tools"
"menu.Other" = "Other"
'''


class LocalizationAuditTests(unittest.TestCase):
    def run_audit(self, missing=False):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            status = audit.audit(missing)
        return status, out.getvalue(), err.getvalue()

    def test_repository_resources_cover_every_registered_title_and_caption(self):
        status, _, errors = self.run_audit()
        self.assertEqual(status, 0, errors)

    def test_unkeyed_command_and_caption_are_reported_with_the_lines_to_add(self):
        with tempfile.TemporaryDirectory(prefix="bareline-l10n-") as temporary:
            root = Path(temporary)
            files = {
                "crates/demo/src/lib.rs": REGISTRATION,
                "crates/app/src/menus.rs": MENUS,
                "crates/commands/src/discovery.rs": DISCOVERY,
                "crates/settings/locales/en.toml": ENGLISH,
            }
            for relative, text in files.items():
                (root / relative).parent.mkdir(parents=True, exist_ok=True)
                (root / relative).write_text(text, encoding="utf-8")
            with mock.patch.multiple(
                audit,
                ROOT=root,
                ENGLISH=root / "crates/settings/locales/en.toml",
                MENUS=root / "crates/app/src/menus.rs",
            ):
                status, _, errors = self.run_audit()
                self.assertEqual(status, 1)
                self.assertIn("command.demo.new: no English resource", errors)
                _, lines, _ = self.run_audit(missing=True)
        self.assertEqual(
            lines.splitlines(),
            ['"command.demo.new" = "Brand New"', '"menu.Fresh Submenu" = "Fresh Submenu"'],
        )
        # Codec names and test-only registrations need no resource.
        self.assertNotIn("encoding.convert.demo", errors + lines)
        self.assertNotIn("test.only", errors + lines)


if __name__ == "__main__":
    unittest.main()
