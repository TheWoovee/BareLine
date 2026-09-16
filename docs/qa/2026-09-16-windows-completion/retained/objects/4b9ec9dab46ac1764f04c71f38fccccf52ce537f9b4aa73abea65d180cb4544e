# SPDX-License-Identifier: MPL-2.0
import hashlib
import json

INITIAL = ''.join(f'row {n:03d} — café 文 🎉\n' for n in range(200))


def fixture():
    data = dict(initial=INITIAL, first=INITIAL + 'PRIMARY', second=INITIAL + 'SECONDARY')
    data['identity'] = dict(procedure='split_clone_sync-v1', fixture_sha256=hashlib.sha256(json.dumps(data, sort_keys=True, ensure_ascii=False).encode()).hexdigest(), encoding='utf-8', eol='lf')
    return data


def validate_observations(steps, records):
    def checkpoint(stage):
        values = [row['details'] for row in records if row.get('stage') == stage]
        if len(values) != 1 or not isinstance(values[0], dict):
            raise ValueError('Split checkpoint missing/duplicate: ' + stage)
        return values[0]
    def pair(stage, expected):
        value = checkpoint(stage)
        panes = value.get('panes')
        if not isinstance(panes, list) or len(panes) != 2 or [p.get('text') for p in panes] != [expected, expected]:
            raise ValueError('Both pane documents must match: ' + stage)
        if any(not p.get('name', '').startswith(f'Pane {n}, ') for n, p in enumerate(panes, 1)):
            raise ValueError('Production pane identities missing')
        if panes[0].get('runtime_id') == panes[1].get('runtime_id'):
            raise ValueError('Distinct pane providers required')
        return panes
    status = {row['id']: row['status'] for row in steps}
    if status.get('s1') == 'PASS':
        pair('split cloned', INITIAL)
    if status.get('s2') == 'PASS':
        for stage, expected in [('split primary edit', fixture()['first']), ('split primary Undo', INITIAL),
                                ('split secondary edit', fixture()['second']), ('split secondary Undo', INITIAL)]:
            pair(stage, expected)
    if status.get('s3') == 'PASS':
        before, after = pair('split scroll before', INITIAL), pair('split scroll after', INITIAL)
        if not after[0].get('focus') or after[1].get('focus'):
            raise ValueError('Synchronized scroll stole focus')
        if not after[0].get('visible') or not after[1].get('visible') or after[0]['visible'].splitlines()[0] != after[1]['visible'].splitlines()[0]:
            raise ValueError('Visible global rows differ')
        if before[0].get('visible') == after[0]['visible']:
            raise ValueError('No observed scrolling')
        if checkpoint('split sync menu').get('checked') is not True:
            raise ValueError('Synchronization was not enabled')
