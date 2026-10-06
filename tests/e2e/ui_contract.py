# SPDX-License-Identifier: MPL-2.0
"""Check native journey selectors against the current product source (QA-03).

The drivers find menu commands by their visible label and UIA controls by
name. Menus, command IDs and banners change between waves, so selectors are
re-derived from the Rust registrations here instead of from old results. This
reads source text only; no application is launched and nothing is compiled.

Usage: python tests/e2e/ui_contract.py
"""
import functools
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
MENUS = "crates/app/src/menus.rs"
ID = r"[a-z][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)+"
# ("id", "Title" ...) and (CommandId("id"), "Title" ...), including wrapped lines.
TUPLE = re.compile(r'\(\s*(?:[\w:]*CommandId\(\s*)?"(' + ID + r')"\s*\)?\s*,\s*"((?:[^"\\]|\\.)*)"')
CONSTANT = re.compile(r'const (\w+): [\w:]*CommandId = [\w:]*CommandId\("(' + ID + r')"\)')
CONSTANT_TUPLE = re.compile(r'\(\s*([A-Z][A-Z0-9_]*)\s*,\s*"((?:[^"\\]|\\.)*)"')
BINDING = re.compile(r'let (\w+) = [\w:]*CommandId\("(' + ID + r')"\)')
SPEC = re.compile(r'CommandSpec\s*\{\s*id:\s*(?:[\w:]*CommandId\("(' + ID + r')"\)|(\w+))\s*,\s*title:\s*"((?:[^"\\]|\\.)*)"')
RELABELLED = re.compile(r'\.entry\(CommandId\("(' + ID + r')"\)\)\s*\.or_default\(\)\s*\.label\s*=')
NOT_APPLICABLE = re.compile(r'CommandId\("(' + ID + r')"\),\s*CommandState::not_applicable')
MENU_CALL = re.compile(r"Regex-(Menu|MenuItem|WaitMenuEnabled|WaitMenuChecked) '([^']+)'")
TEST_MODULE = re.compile(r"#\[cfg\(test\)\]\s*(?:#\[[^\]]*\]\s*)*mod \w+")
# A char literal ('{', '\n', '\u{1F600}'); lifetimes such as 'a are not matched.
CHAR_LITERAL = re.compile(r"'(?:\\u\{[0-9A-Fa-f]{1,6}\}|\\.|[^'\\\n])'")
# UIA names, dialog text and trace stages the drivers depend on, anchored to the
# exact source literal that produces them. A changed literal fails here first.
ANCHORS = (
    ("Find field name", "crates/app/src/find.rs", '"Find",'),
    ("Replace field name", "crates/app/src/find.rs", '"Replace with"'),
    ("Find results status node", "apps/bareline/src/shell/accessibility.rs", 'name: "Find results".into()'),
    ("status label joins name and value", "crates/platform-windows/src/accessibility.rs", 'format!("{}: {value}", item.name)'),
    ("match count wording", "crates/app/src/find.rs", 'format!("{} matches", results.count())'),
    ("single editor provider name", "crates/app/src/accessibility.rs", 'name: "Editor".into()'),
    ("split pane provider names", "apps/bareline/src/shell/accessibility.rs", 'format!("Pane {}, {title}", pane + 1)'),
    ("dirty tab suffix", "apps/bareline/src/shell/views.rs", '", modified"'),
    ("Rust outline rows are 'fn <name>'", "crates/syntax/src/outline.rs", '"fn" => "fn"'),
    ("UDL import report", "crates/app/src/language.rs", 'format!("Imported {} · {} mapping notes"'),
    ("external command consent", "crates/macros/src/process.rs", '"Run this {} command?'),
    ("external consent is a Yes/No message box", "crates/platform-windows/src/process.rs", "MB_YESNO | MB_DEFBUTTON2 | MB_ICONWARNING"),
    ("loaded command relabel", "apps/bareline/src/shell/macros.rs", 'Some(format!("Run {}", definition.name))'),
    ("save prompt instruction", "crates/platform-windows/src/native.rs", 'format!("Save changes to {name}?")'),
    ("Don't Save mnemonic (Alt+N)", "crates/platform-windows/src/native.rs", '(DONT_SAVE_ID, "Do&n\'t Save")'),
    ("Escape cancels the save prompt", "crates/platform-windows/src/native.rs", "TDF_ALLOW_DIALOG_CANCELLATION | TDF_SIZE_TO_CONTENT | TDF_POSITION_RELATIVE_TO_WINDOW"),
    ("F6 switches panes", "apps/bareline/src/shell/views.rs", '("view.focus_other", "Focus Other View", "F6")'),
    ("close trace queued stage", "apps/bareline/src/shell.rs", 'transition(trace_ticket, "queued", "document")'),
    ("close trace busy stage", "apps/bareline/src/shell.rs", '"document-busy"'),
    ("close trace variable", "apps/bareline/src/shell.rs", '"BARELINE_QA_COMMAND_TRACE"'),
    ("first-frame startup event", "apps/bareline/src/shell.rs", '\\"event\\":\\"first_frame\\"'),
)


def label(title):
    """The driver's JourneyInput.Label normalization of a native menu string."""
    return title.replace("&", "").split("\t")[0].rstrip("…. ")


def block_end(text, start):
    """Index after the brace block opening at or after `start`, skipping literals."""
    depth, index = 0, text.index("{", start)
    while index < len(text):
        char = text[index]
        raw = re.match(r'r(#*)"', text[index:index + 8]) if char == "r" and (index == 0 or not (text[index - 1].isalnum() or text[index - 1] == "_")) else None
        if raw:
            index = text.index('"' + raw[1], index + len(raw[0])) + 1 + len(raw[1])
            continue
        if char == '"':
            index += 1
            while text[index] != '"':
                index += 2 if text[index] == "\\" else 1
        elif char == "'" and (literal := CHAR_LITERAL.match(text, index)):
            index = literal.end() - 1
        elif text.startswith("//", index):
            index = text.find("\n", index)
            index = len(text) if index < 0 else index
        elif char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return index + 1
        index += 1
    raise ValueError("Unbalanced test module")


def rust_sources(root):
    """Non-test Rust source text; inline #[cfg(test)] modules are removed."""
    return _rust_sources(Path(root).resolve())


@functools.lru_cache(maxsize=4)
def _rust_sources(root):
    return tuple(_read_sources(root))


def _read_sources(root):
    for top in ("apps", "crates"):
        for path in sorted((root / top).rglob("*.rs")):
            relative = path.relative_to(root).as_posix()
            if "/tests/" in relative or "/examples/" in relative or path.name.endswith("_tests.rs"):
                continue
            text = path.read_text(encoding="utf-8")
            while match := TEST_MODULE.search(text):
                end = text.find(";", match.end())
                brace = text.find("{", match.end())
                if brace < 0 or 0 <= end < brace:
                    text = text[:match.start()] + text[end + 1:]
                else:
                    text = text[:match.start()] + text[block_end(text, brace):]
            yield relative, text


def registrations(root):
    """{command id: {titles}} from every non-test registration in the sources."""
    titles = {}
    for _, text in rust_sources(root):
        constants = dict(CONSTANT.findall(text))
        bindings = dict(BINDING.findall(text))
        for command, title in TUPLE.findall(text):
            titles.setdefault(command, set()).add(title)
        for name, title in CONSTANT_TUPLE.findall(text):
            if name in constants:
                titles.setdefault(constants[name], set()).add(title)
        for command, variable, title in SPEC.findall(text):
            command = command or bindings.get(variable)
            if command:
                titles.setdefault(command, set()).add(title)
    return titles


def string_list(text, name):
    match = re.search(r"pub const " + name + r": &\[&str\] = &\[(.*?)\];", text, re.S)
    if not match:
        raise ValueError(f"{MENUS}: {name} list missing")
    return set(re.findall(r'"(' + ID + r')"', match[1]))


def placement(root):
    text = (root / MENUS).read_text(encoding="utf-8")
    tree = re.search(r"pub const TREE: &\[MenuTemplate\] = &\[(.*?)\n\];", text, re.S)
    if not tree:
        raise ValueError(f"{MENUS}: TREE missing")
    return (set(re.findall(r'C\("(' + ID + r')"\)', tree[1])),
            string_list(text, "WHEN_ENABLED"), string_list(text, "PALETTE_ONLY"))


def contextual(root):
    """(relabelled while applicable, hidden while not applicable) command IDs."""
    relabelled, hidden = set(), set()
    for _, text in rust_sources(root):
        relabelled.update(RELABELLED.findall(text))
        hidden.update(NOT_APPLICABLE.findall(text))
    return relabelled, hidden


def script_labels(path):
    return [(kind, value) for kind, value in MENU_CALL.findall(path.read_text(encoding="utf-8-sig"))]


def check(root=ROOT, scripts=None):
    """Return a list of selector drift errors; empty when every selector resolves."""
    errors = []
    titles = registrations(root)
    tree, when_enabled, palette_only = placement(root)
    relabelled, hidden = contextual(root)
    menu = tree - palette_only
    by_label = {}
    for command, values in titles.items():
        for title in values:
            by_label.setdefault(label(title), set()).add(command)
    scripts = sorted(HERE.glob("native_*.ps1")) if scripts is None else scripts
    for path in scripts:
        for kind, value in script_labels(path):
            where = f"{path.name}: '{value}'"
            candidates = by_label.get(value, set()) & menu
            if not candidates:
                known = sorted(by_label.get(value, set()))
                errors.append(f"{where} matches no menu command" + (f" (registered but not in the menu: {known})" if known else ""))
                continue
            if len(candidates) > 1:
                errors.append(f"{where} is ambiguous in the menu: {sorted(candidates)}")
                continue
            command = next(iter(candidates))
            if command in relabelled:
                errors.append(f"{where} ({command}) is relabelled while it applies; select the shown label")
            if kind == "MenuItem" and command in (when_enabled | hidden):
                errors.append(f"{where} ({command}) is absent until it applies; wait with Regex-Menu/Regex-WaitMenuEnabled")
    if "Exit" not in {label(title) for title in titles.get("app.quit", ())} or "app.quit" not in menu:
        errors.append("native_session.ps1: File > Exit (app.quit) is missing from the menu")
    for description, relative, literal in ANCHORS:
        path = root / relative
        if not path.is_file() or literal not in path.read_text(encoding="utf-8"):
            errors.append(f"{relative}: source anchor for {description} changed ({literal!r})")
    return errors


def main():
    errors = check()
    for error in errors:
        print(error, file=sys.stderr)
    if errors:
        return 1
    print("Native journey selectors match the current command registrations and UIA source anchors.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
