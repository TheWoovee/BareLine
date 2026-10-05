# SPDX-License-Identifier: MPL-2.0
"""Independent multiline capture expectations; no regex engine computes the oracle."""
import base64
import hashlib
import json

INITIAL = 'keep cafe\u0301 \U0001f389\nitem=alpha\nqty=12\nbetween unchanged\nitem=beta\nqty=07\nkeep \u6587\n'
REPLACED = 'keep cafe\u0301 \U0001f389\nalpha:12\nbetween unchanged\nbeta:07\nkeep \u6587\n'
PATTERN = r'(?m)^item=(?<name>[a-z]+)\nqty=(\d+)$'
REPLACEMENT = '${name}:$2'
COUNT = 2
PREVIEW_STATUS = 'Replacement preview status: 2 selected matches; 0 skipped.  Review before applying.'
APPLIED_PREFIX = 'Replacement preview status: Changed 1 open and 0 disk files; replaced 2 matches; skipped 0; failed 0. Receipt: '


def fixture():
    changes = [('item=alpha\nqty=12', 'alpha:12'), ('item=beta\nqty=07', 'beta:07')]
    rows = []
    for before, after in changes:
        offset = len(INITIAL[:INITIAL.index(before)].encode('utf-8'))
        rows.append(f'[\u2713] {offset}: {before.replace(chr(10), " ")} \u2192 {after}')
    data = {'schema_version': 1, 'initial': INITIAL, 'replaced': REPLACED,
            'pattern': PATTERN, 'replacement': REPLACEMENT, 'count': COUNT,
            'preview_rows': rows, 'preview_status': PREVIEW_STATUS, 'applied_prefix': APPLIED_PREFIX}
    digest = hashlib.sha256(json.dumps(data, sort_keys=True, ensure_ascii=False).encode('utf-8')).hexdigest()
    data['identity'] = {'procedure': 'regex_transform-v1', 'encoding': 'utf-8', 'eol': 'lf', 'fixture_sha256': digest}
    return data


def require(condition, message):
    if not condition:
        raise ValueError('Regex transform observation: ' + message)


def preview_header(path):
    return '[\u2713] ' + ('Open document: ' + str(path))[:200]


def validate_observations(steps, records, path):
    require(isinstance(records, list) and len(records) <= 128, 'bounded checkpoint list required')
    def details(stage):
        values = [r.get('details') for r in records if r.get('stage') == stage]
        require(len(values) == 1 and isinstance(values[0], dict), 'missing/duplicate checkpoint ' + stage)
        return values[0]
    def text(stage, expected):
        require(details(stage).get('text') == expected, 'exact text mismatch at ' + stage)
    def saved(stage, expected):
        data, raw = details(stage), expected.encode('utf-8')
        require(data.get('base64') == base64.b64encode(raw).decode('ascii')
                and data.get('sha256') == hashlib.sha256(raw).hexdigest()
                and type(data.get('bytes')) is int and data['bytes'] == len(raw), 'exact bytes mismatch at ' + stage)
    status = {s['id']: s['status'] for s in steps}
    if status.get('s1') == 'PASS':
        text('regex opened exact text', INITIAL)
        text('regex query input', PATTERN)
        mode = details('regex mode')
        require(mode.get('label') == 'Regular Expression Search Mode' and mode.get('checked') is True
                and type(mode.get('state')) is int and mode['state'] & 8, 'regex mode was not observed')
        require(details('regex match count').get('name') == 'Find results: 2 matches', 'complete exact count required')
        text('regex setup preserved text', INITIAL)
        saved('regex setup preserved bytes', INITIAL)
    if status.get('s2') == 'PASS':
        text('regex replacement input', REPLACEMENT)
        preview = details('regex capture preview')
        require(preview.get('status', {}).get('name') == PREVIEW_STATUS, 'complete selected preview count required')
        rows = preview.get('rows')
        require(isinstance(rows, list) and len(rows) == 3, 'one document and two preview rows required')
        require([r.get('name') for r in rows] == [preview_header(path), *fixture()['preview_rows']], 'expanded capture preview mismatch')
        require(all(r.get('control_type') == 'ControlType.ListItem' for r in rows), 'actual preview rows required')
        text('regex preview preserved text', INITIAL)
        saved('regex preview preserved bytes', INITIAL)
        applied = details('regex replacement count').get('name', '')
        require(applied.startswith(APPLIED_PREFIX) and len(applied) > len(APPLIED_PREFIX), 'successful exact applied count required')
        text('regex replaced exact text', REPLACED)
        saved('regex unsaved replacement preserved disk', INITIAL)
        saved('regex saved replacement bytes', REPLACED)
    if status.get('s3') == 'PASS':
        text('regex single Undo', INITIAL)
        saved('regex Undo before save preserved disk', REPLACED)
        saved('regex restored original bytes', INITIAL)
