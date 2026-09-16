# SPDX-License-Identifier: MPL-2.0
import hashlib
import json
from pathlib import Path
import sys
import tomllib

INITIAL = 'alpha target omega\nsecond line\n'
FINAL = 'REC alpha FOUND omega\nsecond line\n'
ARGS = ['space value', 'a&b|c^d%PATH%', 'quote"value', '文🎉', '']


def fixture():
    data = dict(initial=INITIAL, final=FINAL, query='target', prefix='REC ', replacement='FOUND', argv=ARGS)
    data['identity'] = dict(procedure='macro_external-v1', fixture_sha256=hashlib.sha256(json.dumps(data, sort_keys=True, ensure_ascii=False).encode()).hexdigest(), encoding='utf-8', eol='lf')
    return data


def external_definition(scratch):
    args = [str(Path(__file__).with_name('external_fixture.py').resolve()), str(scratch), str(scratch / 'macro-source.txt'), *ARGS]
    quote = lambda value: json.dumps(value, ensure_ascii=False)
    return 'format_version=1\nname="QA owned command"\nprogram='+quote(sys.executable)+'\nargs=['+','.join(map(quote,args))+']\nmode="direct"\ncapture=true\n'


def validate_macro(raw):
    value = tomllib.loads(raw.decode())
    events = value.get('events', [])
    if value.get('format_version') != 1 or not 1 <= len(events) <= 64: raise ValueError('Invalid recorded macro')
    searches = [e for e in events if e.get('command') == 'search.find_next']
    if len(searches) != 1 or searches[0].get('arguments', {}).get('pattern') != 'target' or searches[0]['arguments'].get('mode') != 'literal':
        raise ValueError('Recorded search arguments missing or changed')
    typed = ''.join(e.get('text', e.get('arguments', {}).get('text', '')) for e in events if e.get('command') == 'edit.insert_text')
    if typed != 'REC FOUND': raise ValueError('Recorded edit arguments differ')


def validate_observations(steps, records, scratch):
    def check(stage):
        found = [r['details'] for r in records if r.get('stage') == stage]
        if len(found) != 1 or not isinstance(found[0], dict): raise ValueError('Macro checkpoint missing/duplicate: '+stage)
        return found[0]
    status = {r['id']: r['status'] for r in steps}
    if status.get('s1') == 'PASS':
        if check('macro recorded result').get('text') != FINAL: raise ValueError('Recorded document differs')
        validate_macro((scratch / 'local/Bareline/macros/macro-01.toml').read_bytes())
    if status.get('s2') == 'PASS':
        if check('macro replayed result').get('text') != FINAL or check('owned exit 1').get('exit_code') != 0:
            raise ValueError('Restarted macro result differs')
    if status.get('s3') == 'PASS':
        value = check('external literal argv')
        if value.get('argv') != ARGS or value.get('cwd') != str(scratch): raise ValueError('Literal arguments or directory changed')
        navigation = check('external output navigation')
        if navigation.get('text') != FINAL or navigation.get('caret_utf16') != FINAL.index('\n') + 1: raise ValueError('Output link navigation differs')
        cleanup = check('external cancelled tree')
        if cleanup.get('parent_exited') is not True or cleanup.get('child_exited') is not True or cleanup.get('parent_id') != value.get('pid') or cleanup.get('child_id') != value.get('child_pid'):
            raise ValueError('Owned external process tree termination unobserved')
