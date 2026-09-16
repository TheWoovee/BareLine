# SPDX-License-Identifier: MPL-2.0
import hashlib
import json
import code_config_fixture

XML = '<NotepadPlus><UserLang name="QA Fixture" ext="qaudl"><KeywordLists><Keywords name="Keywords1">sentinel</Keywords><Keywords name="Operators1">+</Keywords></KeywordLists></UserLang></NotepadPlus>'
SAMPLE = 'sentinel "hello" 42 + plain\n'


def fixture(theme):
    data = dict(xml=XML, initial=SAMPLE, file_name='sample.qaudl',
                import_status='Imported QA Fixture · 3 mapping notes',
                normal_rgb=code_config_fixture.fixture(theme)['normal_rgb'],
                probes=[dict(kind=kind, text=text, rgb=code_config_fixture.COLORS[theme][kind])
                        for kind, text in [('keyword', 'sentinel'), ('string', 'hello'), ('number', '42')]])
    data['identity'] = dict(procedure='udl-v1', fixture_sha256=hashlib.sha256(json.dumps(data, sort_keys=True, ensure_ascii=False).encode()).hexdigest(), encoding='utf-8', eol='lf')
    return data


def validate_observations(steps, records, theme):
    def checkpoint(stage):
        found = [r['details'] for r in records if r.get('stage') == stage]
        if len(found) != 1 or not isinstance(found[0], dict):
            raise ValueError('UDL checkpoint missing/duplicate: ' + stage)
        return found[0]
    status = {row['id']: row['status'] for row in steps}
    if status.get('s1') == 'PASS':
        if checkpoint('udl import report').get('name') != fixture(theme)['import_status']:
            raise ValueError('UDL import report differs')
        installed = checkpoint('udl installed definition')
        if installed.get('id') != 'qa-fixture' or installed.get('keywords') != ['sentinel'] or installed.get('extensions') != ['qaudl']:
            raise ValueError('UDL installed definition differs')
    for step, stage in [('s2', 'udl sample'), ('s3', 'udl restarted sample')]:
        if status.get(step) == 'PASS' and checkpoint(stage).get('text') != SAMPLE:
            raise ValueError('UDL sample changed')
    if status.get('s2') == 'PASS':
        code_config_fixture.validate_colors(checkpoint('code highlighting')['probes'], theme, fixture(theme))
    if status.get('s3') == 'PASS':
        if checkpoint('owned exit 1').get('exit_code') != 0 or checkpoint('owned launch 2').get('pid') == checkpoint('owned launch 1').get('pid'):
            raise ValueError('UDL restart was not observed')
        code_config_fixture.validate_colors(checkpoint('udl restarted highlighting')['probes'], theme, fixture(theme))
