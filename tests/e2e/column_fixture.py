# SPDX-License-Identifier: MPL-2.0
"""Literal display-column oracle, independent of the product's column mapper."""
import base64
import hashlib
import json

INITIAL = 'a\tZ\n文abZ\nx\n'
INSERTED = 'a\t|Z\n文ab|Z\nx   |\n'
POSITIONS = [2, 7, 10]  # UTF-16 insertion offsets, before padding the short line.


def fixture():
    data = dict(initial=INITIAL, inserted=INSERTED, text='|', column=4,
                tab_width=4, selection_offsets=POSITIONS)
    digest = hashlib.sha256(json.dumps(data, sort_keys=True, ensure_ascii=False).encode()).hexdigest()
    data['identity'] = dict(procedure='column_multi_cursor-v1', fixture_sha256=digest,
                            encoding='utf-8', eol='lf', tab_width=4)
    return data


def validate_observations(steps, records):
    def require(value, message):
        if not value:
            raise ValueError('Column observation: ' + message)
    require(isinstance(records, list) and len(records) <= 128, 'bounded checkpoints required')
    def checkpoint(stage):
        matches = [row.get('details') for row in records if row.get('stage') == stage]
        require(len(matches) == 1 and isinstance(matches[0], dict), 'missing/duplicate ' + stage)
        return matches[0]
    def text(stage, expected):
        require(checkpoint(stage).get('text') == expected, 'exact text ' + stage)
    def saved(stage, expected):
        value, raw = checkpoint(stage), expected.encode('utf-8')
        require(value.get('base64') == base64.b64encode(raw).decode()
                and value.get('sha256') == hashlib.sha256(raw).hexdigest()
                and type(value.get('bytes')) is int and value['bytes'] == len(raw), 'exact bytes ' + stage)
    status = {s['id']: s['status'] for s in steps}
    if status.get('s1') == 'PASS':
        text('column opened', INITIAL)
        selection = checkpoint('column selections')
        require(selection.get('text') == INITIAL, 'selection must preserve text')
        require(selection.get('selections') == [dict(start_utf16=n, text='') for n in POSITIONS],
                'three exact display-column selections required')
        saved('column original disk', INITIAL)
    if status.get('s2') == 'PASS':
        require(checkpoint('column insertion input').get('text') == '|', 'literal input required')
        text('column inserted', INSERTED)
        saved('column unsaved disk', INITIAL)
        saved('column inserted disk', INSERTED)
    if status.get('s3') == 'PASS':
        text('column one Undo', INITIAL)
        saved('column Undo disk unchanged', INSERTED)
        saved('column restored disk', INITIAL)
