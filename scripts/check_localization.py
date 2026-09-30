# SPDX-License-Identifier: MPL-2.0
"""Localization readiness audit (BIZ-30); run from any directory using Python 3.11+.

Checks, without compiling, that every command title and menu caption in the
registration and menu tables has an English resource in
`crates/settings/locales/en.toml` with the same text. The Rust tests
(`every_command_and_menu_has_an_english_resource` in the shell inventory, and
the crate-level checks in `bareline-settings` and `bareline-app`) enforce the
same rule against the real registry; this script is the fast pre-check and
names the exact lines to add.

    python scripts/check_localization.py              # audit; exit 1 on gaps
    python scripts/check_localization.py --missing    # print TOML lines to add
    python scripts/check_localization.py --inventory  # remaining literal sites per module
"""
import argparse
import pathlib
import re
import sys
import tomllib

ROOT = pathlib.Path(__file__).resolve().parents[1]
ENGLISH = ROOT / "crates/settings/locales/en.toml"
MENUS = ROOT / "crates/app/src/menus.rs"
SOURCES = ("crates", "apps")
ID = r"[a-z][A-Za-z0-9_]*(?:\.[A-Za-z0-9_\-]+)+"
STRING = r'"((?:[^"\\]|\\.)*)"'
TOKEN = re.compile(r"""r(#*)"|"(?:[^"\\]|\\.)*"|'(?:\\.|[^\\'])'|//[^\n]*|/\*.*?\*/|[{}]""", re.S)
TEST_MODULE = re.compile(r"#\[cfg\(test\)\]\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*\{")
# Codec commands are titled with the encoding's standard name
# (`Encoding::display_name`), which is not translated.
EXEMPT = re.compile(r"^encoding\.(?:interpret|convert)\.")


def unescape(text):
    return re.sub(r"\\(.)", lambda m: {"n": "\n", "t": "\t"}.get(m.group(1), m.group(1)), text)


def block_end(text, start):
    """Index just past the brace block whose `{` is at `start`."""
    depth, index = 0, start
    while True:
        token = TOKEN.search(text, index)
        if token is None:
            return len(text)
        if token.group(1) is not None:
            index = text.index('"' + token.group(1), token.end()) + 1 + len(token.group(1))
            continue
        if token.group(0) == "{":
            depth += 1
        elif token.group(0) == "}":
            depth -= 1
            if depth == 0:
                return token.end()
        index = token.end()


def without_tests(text):
    parts, index = [], 0
    while (match := TEST_MODULE.search(text, index)) is not None:
        parts.append(text[index:match.start()])
        index = block_end(text, match.end() - 1)
    parts.append(text[index:])
    return "".join(parts)


def sources():
    for base in SOURCES:
        for path in sorted((ROOT / base).rglob("*.rs")):
            relative = path.relative_to(ROOT)
            if {"tests", "target", "benches"} & set(relative.parts) or path.name.endswith("_tests.rs"):
                continue
            yield relative.as_posix(), without_tests(path.read_text(encoding="utf-8"))


def string_consts(text):
    """`const NAME: [..] = [..];` arrays in a file, as lists of string tuples."""
    consts = {}
    for match in re.finditer(r"const\s+([A-Z][A-Z0-9_]*)\s*:[^=]*=\s*\[(.*?)\];", text, re.S):
        body = match.group(2)
        tuples = [re.findall(STRING, item) for item in re.findall(r"\(([^()]*)\)", body)]
        consts[match.group(1)] = tuples if tuples else [[value] for value in re.findall(STRING, body)]
    for match in re.finditer(r"const\s+([A-Z][A-Z0-9_]*)\s*:\s*[\w:]*CommandId\s*=\s*[\w:]*CommandId\(\"(" + ID + r")\"\)", text):
        consts[match.group(1)] = [[match.group(2)]]
    return consts


def functions(text):
    """Bodies of functions that build a CommandSpec, directly or through a helper in the same file."""
    bodies = {}
    for match in re.finditer(r"\bfn\s+(\w+)", text):
        brace = text.find("{", match.end())
        if brace != -1:
            bodies.setdefault(match.group(1), []).append(text[brace:block_end(text, brace)])
    builders = {name for name, found in bodies.items() if any("CommandSpec" in body for body in found)}
    for found in bodies.values():
        for body in found:
            if "CommandSpec" in body or any(re.search(r"\b" + name + r"\(", body) for name in builders):
                yield body


def command_titles():
    """Statically registered `id -> [(title, file)]` from functions building a CommandSpec."""
    found = {}
    for relative, text in sources():
        if "CommandSpec" not in text:
            continue
        consts = string_consts(text)
        for body in functions(text):
            pairs = []
            pairs += re.findall(r"(?<![!\w])\(\s*\"(" + ID + r")\"\s*,\s*" + STRING, body)
            pairs += re.findall(r"\(\s*(?:[\w:]*::)?CommandId\(\"(" + ID + r")\"\)\s*,\s*" + STRING, body)
            pairs += re.findall(r"id:\s*(?:[\w:]*::)?CommandId\(\"(" + ID + r")\"\)\s*,\s*title:\s*" + STRING, body)
            for name, value in re.findall(r"let\s+(\w+)\s*=\s*(?:[\w:]*::)?CommandId\(\"(" + ID + r")\"\);", body):
                title = re.search(r"CommandSpec\s*\{\s*id(?::\s*" + name + r")?\s*,\s*title:\s*" + STRING, body)
                if title:
                    pairs.append((value, title.group(1)))
            for name, title in re.findall(r"\(\s*([A-Z][A-Z0-9_]*)\s*,\s*" + STRING, body):
                pairs += [(value[0], title) for value in consts.get(name, []) if value]
            for loop in re.finditer(r"for\s+\w+\s+in\s+([A-Z][A-Z0-9_]*)\s*\{", body):
                block = body[loop.end() - 1:block_end(body, loop.end() - 1)]
                fixed = re.findall(r"title:\s*" + STRING, block)
                if len(fixed) == 1:
                    pairs += [(value[0], fixed[0]) for value in consts.get(loop.group(1), [])]
            for name, prefix in re.findall(r"([A-Z][A-Z0-9_]*)\s*\.into_iter\(\)\s*\.enumerate\(\)\s*\.map\(\|\(i,\s*\w+\)\|\s*\(\w+,\s*format!\(\"([^\"{]*)\{\}\",\s*i\s*\+\s*1\)", body):
                pairs += [(value[0], f"{prefix}{index + 1}") for index, value in enumerate(consts.get(name, []))]
            for name, pattern in re.findall(r"([A-Z][A-Z0-9_]*)\.map\(\|\(([\w\s,]+)\)\|", body):
                names = [part.strip() for part in pattern.split(",")]
                if "title" in names:
                    index = names.index("title")
                    pairs += [(value[0], value[index]) for value in consts.get(name, []) if len(value) > index]
            for command, title in pairs:
                if re.fullmatch(ID, title) or title.startswith("#"):
                    continue  # a token or colour default, not a title
                found.setdefault(command, set()).add((unescape(title), relative))
    return {command: titles for command, titles in found.items() if not EXEMPT.match(command)}


def menu_captions():
    text = MENUS.read_text(encoding="utf-8")
    captions = set(re.findall(r"\bSub\(\s*" + STRING, text))
    discovery = (ROOT / "crates/commands/src/discovery.rs").read_text(encoding="utf-8")
    taxonomy = re.search(r"MENU_TAXONOMY:[^=]*=\s*\[(.*?)\];", discovery, re.S)
    captions |= set(re.findall(STRING, taxonomy.group(1)))
    captions |= set(re.findall(r"OTHER_MENU:\s*&str\s*=\s*" + STRING, discovery))
    # Declared menu paths such as "Encoding > Convert To" add submenus of their own.
    for _, text in sources():
        for path in re.findall(r"\"((?:[A-Z][\w ]*\s*>\s*)+[A-Z][\w ]*)\"", text):
            captions |= {part.strip() for part in path.split(">")}
    return captions


def toml_line(key, value):
    escaped = value.replace("\\", "\\\\").replace('"', '\\"').replace("{", "{{").replace("}", "}}")
    return f'"{key}" = "{escaped}"'


def audit(emit):
    messages = tomllib.loads(ENGLISH.read_text(encoding="utf-8"))["messages"]
    shown = {key: value.replace("{{", "{").replace("}}", "}") for key, value in messages.items()}
    problems, lines = [], []
    for command, titles in sorted(command_titles().items()):
        key = f"command.{command}"
        values = {title for title, _ in titles}
        if len(values) > 1:
            problems.append(f"{key}: registered with several titles {sorted(values)}")
            continue
        title, where = next(iter(titles))
        if key not in shown:
            problems.append(f"{key}: no English resource (registered in {where})")
            lines.append(toml_line(key, title))
        elif shown[key] != title:
            problems.append(f"{key}: resource {shown[key]!r} differs from the registered title {title!r} ({where})")
            lines.append(toml_line(key, title))
    for caption in sorted(menu_captions()):
        key = f"menu.{caption}"
        if key not in shown:
            problems.append(f"{key}: no English resource")
            lines.append(toml_line(key, caption))
    if emit:
        print("\n".join(lines))
        return 0
    for problem in problems:
        print(problem, file=sys.stderr)
    if problems:
        print(f"{len(problems)} localization gap(s); `--missing` prints the lines to add to {ENGLISH.relative_to(ROOT).as_posix()}", file=sys.stderr)
        return 1
    print(f"{len(messages)} English resources cover every statically registered command title and menu caption")
    return 0


# A literal a person reads: starts with a capital letter or digit and holds a space
# or ends with punctuation, and is not a path, key, colour or format directive.
PROSE = re.compile(r'"([A-Z][^"\\{}]*?(?: |[.…?!:])[^"\\]*)"')
NOT_PROSE = re.compile(r"^(?:[A-Z][A-Za-z0-9]+(?:\+[A-Za-z0-9]+)+|[A-Z]:\\|#[0-9A-Fa-f]+)$")


def resourced():
    """Literals already served from keyed resources: en.toml messages and the
    setting titles and descriptions `LocalePack::english` keys by setting."""
    values = set(tomllib.loads(ENGLISH.read_text(encoding="utf-8"))["messages"].values())
    model = (ROOT / "crates/settings/src/model.rs").read_text(encoding="utf-8")
    for arguments in re.findall(r"setting!\(\s*" + STRING + r"\s*,\s*" + STRING + r"\s*,\s*" + STRING, model):
        values |= {unescape(arguments[1]), unescape(arguments[2])}
    return values


def inventory(minimum):
    """Markdown tables of literal UI string sites not yet served from resources."""
    covered = resourced()
    crates, modules = {}, {}
    for relative, text in sources():
        text = re.sub(r"(?m)^\s*//[^\n]*", "", text)
        found = [value for value in PROSE.findall(text) if not NOT_PROSE.match(value) and unescape(value) not in covered]
        if not found:
            continue
        parts = relative.split("/")
        crate = "/".join(parts[:2])
        module = "::".join([parts[1].replace("-", "_")] + [part.removesuffix(".rs") for part in parts[3:] if part not in ("lib.rs", "main.rs")])
        crates[crate] = crates.get(crate, 0) + len(found)
        modules[module] = modules.get(module, 0) + len(found)
    ranked = lambda counts: sorted(counts.items(), key=lambda item: (-item[1], item[0]))
    print("| Crate | Literal sites |")
    print("|---|---:|")
    for crate, count in ranked(crates):
        print(f"| `{crate}` | {count} |")
    print(f"| **Total** | **{sum(crates.values())}** |")
    print()
    print(f"| Module (≥ {minimum} sites) | Literal sites |")
    print("|---|---:|")
    rest = 0
    for module, count in ranked(modules):
        if count >= minimum:
            print(f"| `{module}` | {count} |")
        else:
            rest += count
    print(f"| Other modules | {rest} |")
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--missing", action="store_true", help="print the resource lines to add")
    parser.add_argument("--inventory", action="store_true", help="count remaining literal UI string sites")
    parser.add_argument("--minimum", type=int, default=20, help="smallest module listed by --inventory")
    arguments = parser.parse_args()
    # Titles carry "…"; a Windows console code page cannot print it.
    sys.stdout.reconfigure(encoding="utf-8")
    sys.stderr.reconfigure(encoding="utf-8")
    return inventory(arguments.minimum) if arguments.inventory else audit(arguments.missing)


if __name__ == "__main__":
    sys.exit(main())
