# SPDX-License-Identifier: MPL-2.0
import hashlib
import json

INITIAL = '// fixture\nfn alpha() {}\n\nfn target() { let value = 7; }\n'


def fixture():
    data = dict(initial=INITIAL, notes='owned workspace notes\n', target_offset=INITIAL.index('target'))
    data['identity'] = dict(procedure='workspace-v1', fixture_sha256=hashlib.sha256(json.dumps(data, sort_keys=True).encode()).hexdigest(), encoding='utf-8', eol='lf')
    return data


def validate_observations(steps, records):
    def checkpoint(stage):
        values = [r['details'] for r in records if r.get('stage') == stage]
        if len(values) != 1 or not isinstance(values[0], dict):
            raise ValueError('Workspace checkpoint missing/duplicate: ' + stage)
        return values[0]
    status = {r['id']: r['status'] for r in steps}
    if status.get('s1') == 'PASS' and checkpoint('workspace tree').get('names') != ['main.rs', 'notes.txt', 'workspace-fixture']:
        raise ValueError('Workspace must contain only generated entries')
    if status.get('s2') == 'PASS':
        nav = checkpoint('workspace target')
        if nav.get('text') != INITIAL or nav.get('caret_utf16') != fixture()['target_offset']:
            raise ValueError('Outline must navigate to exact target')
    if status.get('s3') == 'PASS':
        if checkpoint('workspace renamed tree').get('names') != ['notes.txt', 'renamed.rs', 'workspace-fixture']:
            raise ValueError('Renamed tree differs')
        nav = checkpoint('workspace renamed target')
        if nav.get('text') != INITIAL or nav.get('caret_utf16') != fixture()['target_offset']:
            raise ValueError('Renamed document navigation differs')
        if checkpoint('workspace rename paths').get('old_exists') is not False:
            raise ValueError('Old filename still exists')
