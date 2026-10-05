# SPDX-License-Identifier: MPL-2.0
import base64
import hashlib
import json

DOCUMENT = 'Portable session café 文\n'
NEW_TEXT = 'Relocated 文'


def fixture(theme):
    settings = f"schema_version=1\n[theme]\nmode='{theme}'\n[files]\ndefault_encoding='utf-16be'\ndefault_eol='lf'\nautosave_seconds=0\n"
    data = dict(settings=settings, document=DOCUMENT, new_text=NEW_TEXT)
    data['identity'] = dict(procedure='portable-v1', fixture_sha256=hashlib.sha256(json.dumps(data, sort_keys=True, ensure_ascii=False).encode()).hexdigest(), source='pinned executable in isolated generated portable layout')
    return data


def validate_observations(steps, records, theme):
    def check(stage):
        found = [r['details'] for r in records if r.get('stage') == stage]
        if len(found) != 1 or not isinstance(found[0], dict): raise ValueError('Portable checkpoint missing/duplicate: ' + stage)
        return found[0]
    def bytes_check(stage, raw):
        row = check(stage)
        if row.get('base64') != base64.b64encode(raw).decode() or row.get('sha256') != hashlib.sha256(raw).hexdigest() or type(row.get('bytes')) is not int or row['bytes'] != len(raw):
            raise ValueError('Portable exact saved bytes differ: ' + stage)
    status = {r['id']: r['status'] for r in steps}
    if status.get('s1') == 'PASS':
        bytes_check('portable settings edited', fixture(theme)['settings'].encode())
        if check('portable containment before').get('external_profile_files') != []: raise ValueError('Portable state escaped profile')
    if status.get('s2') == 'PASS':
        move = check('portable relocation')
        if move.get('old_exists') is not False or move.get('inventory_before') != move.get('inventory_after') or not move.get('inventory_before'):
            raise ValueError('Complete portable directory was not moved unchanged')
        if check('owned exit 1').get('exit_code') != 0: raise ValueError('Portable close failed')
    if status.get('s3') == 'PASS':
        if check('portable restored session').get('text') != DOCUMENT: raise ValueError('Portable session did not restore')
        bytes_check('portable relocated encoding', b'\xfe\xff' + NEW_TEXT.encode('utf-16be'))
        if check('portable containment after').get('external_profile_files') != []: raise ValueError('Relocated state escaped profile')
        if check('owned launch 1').get('pid') == check('owned launch 2').get('pid'): raise ValueError('No fresh relocated launch')
