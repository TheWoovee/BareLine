# SPDX-License-Identifier: MPL-2.0
"""Independent expected fixture and native observation checks for code_config."""
import base64
import hashlib
import json
import math

INITIAL = '// Unicode cafe\u0301 \U0001f389 stays.\nfn main() {\n    let answer_value = 42;\n    let message = "hello";\n}'
INDENTED = INITIAL[:-2] + '\n    \n}'
PREFIX = INDENTED[:-2] + 'ans\n}'
COMPLETED = INDENTED[:-2] + 'answer_value\n}'
FINAL = INDENTED[:-2] + 'answer_value;\n}'
COLORS = {
    'dark': {'keyword': 0xC79BFF, 'string': 0xA5D6A7, 'number': 0xF5B76B, 'comment': 0x9AA3AD},
    'light': {'keyword': 0x7B3FB5, 'string': 0x2E7D32, 'number': 0xB45309, 'comment': 0x5C6570},
}


def fixture(theme):
    data = {'schema_version': 1, 'language': 'Rust', 'file_name': 'code-config.rs',
            'initial': INITIAL, 'indented': INDENTED, 'prefix': PREFIX, 'completed': COMPLETED, 'final': FINAL,
            'candidate': 'answer_value', 'normal_rgb': 0xE6E8EA if theme == 'dark' else 0x23272B,
            'probes': [{'kind': kind, 'text': value, 'rgb': COLORS[theme][kind]}
                       for kind, value in [('keyword', 'fn'), ('string', 'hello'), ('number', '42'), ('comment', 'Unicode')]]}
    digest = hashlib.sha256(json.dumps(data, sort_keys=True, ensure_ascii=False).encode('utf-8')).hexdigest()
    data['identity'] = {'procedure': 'code_config-v1', 'language': 'Rust', 'encoding': 'utf-8', 'eol': 'lf',
                        'fixture_sha256': digest}
    return data


def require(condition, message):
    if not condition:
        raise ValueError('Code/config observation: ' + message)


def near(a, b):
    return all(abs(((a >> shift) & 255) - ((b >> shift) & 255)) <= 12 for shift in (16, 8, 0))


def validate_colors(probes, theme, expected=None):
    expected = expected or fixture(theme)
    require(isinstance(probes, list) and len(probes) == len(expected['probes']), 'exact token color probes required')
    require({p.get('kind') for p in probes} == {p['kind'] for p in expected['probes']}, 'token categories missing or duplicated')
    for wanted in expected['probes']:
        actual = next(p for p in probes if p['kind'] == wanted['kind'])
        require(actual.get('text') == wanted['text'], 'token text mismatch')
        bounds = actual.get('bounds', [])
        require(isinstance(bounds, list) and 1 <= len(bounds) <= 32, 'token geometry missing or excessive')
        require(all(isinstance(r, list) and len(r) == 4 and all(type(v) in (int, float) and math.isfinite(v) for v in r)
                    and r[2] > 0 and r[3] > 0 for r in bounds), 'invalid token rectangle')
        histogram = actual.get('histogram', [])
        require(isinstance(histogram, list) and 1 <= len(histogram) <= 8192, 'pixel histogram missing or excessive')
        seen = set()
        total = matched = normal = 0
        for color in histogram:
            rgb, count = color.get('rgb'), color.get('count')
            require(type(rgb) is int and 0 <= rgb <= 0xFFFFFF and rgb not in seen, 'invalid/duplicate RGB')
            require(type(count) is int and 1 <= count <= 8192, 'invalid pixel count')
            seen.add(rgb)
            total += count
            matched += count if near(rgb, wanted['rgb']) else 0
            normal += count if near(rgb, expected['normal_rgb']) else 0
        require(3 <= total <= 8192, 'pixel area outside bounds')
        require(matched >= 3 and matched > normal, wanted['kind'] + ' is not painted with its expected token color')


def validate_observations(steps, records, theme):
    """A PASS needs the actual checkpoint content, not just PASS-shaped prose."""
    require(isinstance(records, list) and len(records) <= 128, 'bounded checkpoint list required')
    def details(stage):
        matches = [r.get('details') for r in records if r.get('stage') == stage]
        require(len(matches) == 1 and isinstance(matches[0], dict), 'missing/duplicate checkpoint ' + stage)
        return matches[0]
    def text(stage, expected):
        require(details(stage).get('text') == expected, 'exact text mismatch at ' + stage)
    def saved(stage, expected):
        data = details(stage)
        raw = expected.encode('utf-8')
        require(data.get('base64') == base64.b64encode(raw).decode('ascii')
                and data.get('sha256') == hashlib.sha256(raw).hexdigest()
                and data.get('bytes') == len(raw), 'exact bytes mismatch at ' + stage)
    status = {s['id']: s['status'] for s in steps}
    if status.get('s1') == 'PASS':
        text('code opened exact text', INITIAL)
        require(details('code language')['name'] == 'Language: Rust', 'selected language mismatch')
        validate_colors(details('code highlighting')['probes'], theme)
        saved('code source initially unchanged', INITIAL)
    if status.get('s2') == 'PASS':
        text('code smart indentation', INDENTED)
        text('code completion prefix', PREFIX)
        popup = details('code completion selected')
        require(popup.get('name') == 'answer_value' and popup.get('selected') is True
                and popup.get('control_type') == 'ControlType.ListItem', 'completion selection not observed')
        text('code completion accepted', COMPLETED)
        text('code completion Undo', PREFIX)
        text('code completion Redo', COMPLETED)
        text('code final edit', FINAL)
        saved('code unsaved edits preserve disk', INITIAL)
    if status.get('s3') == 'PASS':
        saved('code saved exact bytes', FINAL)
        text('code reopened exact text', FINAL)
        saved('code reopened bytes unchanged', FINAL)
