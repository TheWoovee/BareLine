# SPDX-License-Identifier: MPL-2.0
"""Selector drift checks read source text only; no editor is launched."""
from pathlib import Path
import tempfile
import tomllib
import unittest

import macro_fixture
import ui_contract
import workspace_fixture


class UiContractTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)

    def script(self, text):
        path = self.root / "native_fixture.ps1"
        path.write_text(text, encoding="utf-8")
        return [path]

    def test_current_journey_selectors_resolve(self):
        self.assertEqual(ui_contract.check(), [])

    def test_renamed_ambiguous_and_relabelled_commands_are_reported(self):
        # 'Normal Search Mode' was renamed Literal Search Mode in wave 2.
        errors = ui_contract.check(scripts=self.script("Regex-Menu 'Normal Search Mode'\n"))
        self.assertEqual(len(errors), 1)
        self.assertIn("matches no menu command", errors[0])
        # A loaded definition replaces the static title with "Run <name>".
        errors = ui_contract.check(scripts=self.script("Regex-Menu 'Run Loaded External Command'\n"))
        self.assertIn("relabelled", errors[0])
        # Reopen and Follow is listed only while it applies.
        errors = ui_contract.check(scripts=self.script("$x=Regex-MenuItem 'Reopen and Follow'\n"))
        self.assertIn("absent until it applies", errors[0])
        self.assertEqual(ui_contract.check(scripts=self.script("Regex-Menu 'Reopen and Follow'\n")), [])

    def test_labels_normalize_like_the_native_driver(self):
        self.assertEqual(ui_contract.label("Open Workspace Folder…"), "Open Workspace Folder")
        self.assertEqual(ui_contract.label("Do&n't Save\tAlt+N"), "Don't Save")

    def test_test_modules_and_literals_do_not_hide_registrations(self):
        source = self.root / "crates/demo/src/lib.rs"
        source.parent.mkdir(parents=True)
        source.write_text('#[cfg(test)]\nmod tests {\n    const BRACE: char = \'{\';\n    fn f() { let _ = ("demo.test", "Test Only"); let _ = r#"}"#; }\n}\n'
                          'pub fn register() { let _ = [("demo.real", "Real Title…")]; }\n', encoding="utf-8")
        titles = ui_contract.registrations(self.root)
        self.assertEqual(titles, {"demo.real": {"Real Title…"}})

    def test_dynamic_selectors_follow_their_fixtures(self):
        # The macro driver selects "Run <name>" from the generated definition.
        self.assertIn("Regex-Menu ('Run '+$script:macroFixture.command_name)",
                      (ui_contract.HERE / "native_macro.ps1").read_text(encoding="utf-8"))
        self.assertEqual(tomllib.loads(macro_fixture.external_definition(self.root))["name"], macro_fixture.COMMAND_NAME)
        # Rust outline rows are "<kind> <name>" and the Rust kind is "fn".
        self.assertEqual(workspace_fixture.fixture()["target_symbol"], "fn target")


if __name__ == "__main__":
    unittest.main()
