# SPDX-License-Identifier: MPL-2.0
"""Focused UI regressions for open manual-QA defects (QA-15).

Each step starts its own owned editor, so one failure does not hide the other
defects. The oracles below check retained observations; they are not records
of a native product run.
"""
import hashlib
import json

INITIAL = 'regression fixture alpha\nsecond needle line\nthird line\n'
QUERY = 'needle'
UNTITLED = 'unsaved ISSUE-008 text'
# Large enough that a paged save is still running when the next chord arrives.
BUSY_BYTES = 256 * 1024 * 1024
BUSY_LINE = '0123456789abcdef' * 3 + '0123456789abcde\n'
BUSY_EDIT = 'X'
RELAUNCHES = 2


def fixture():
    data = dict(initial=INITIAL, query=QUERY, find_status='Find results: 1 match', untitled=UNTITLED,
                split_edited='A' + INITIAL + 'Z', busy_bytes=BUSY_BYTES, busy_line=BUSY_LINE, busy_edit=BUSY_EDIT,
                relaunches=RELAUNCHES)
    data['identity'] = dict(procedure='ui_regressions-v1', fixture_sha256=hashlib.sha256(json.dumps(data, sort_keys=True, ensure_ascii=False).encode()).hexdigest(), encoding='utf-8', eol='lf')
    return data


def generate_busy(path, size=BUSY_BYTES):
    """Write the paged busy-save fixture in bounded chunks; never overwrites."""
    line = BUSY_LINE.encode()
    if size < len(line) or size % len(line):
        raise ValueError('Busy fixture size must be a whole number of lines')
    block = line * 1024
    with path.open('xb') as stream:
        remaining = size
        while remaining:
            chunk = block[:min(remaining, len(block))]
            stream.write(chunk)
            remaining -= len(chunk)
    return dict(bytes=size)


def busy_close_deferred(rows):
    """True when one document close ticket was queued and then deferred as busy."""
    if not isinstance(rows, list):
        return False
    stages = {}
    for row in rows:
        if isinstance(row, dict) and row.get('event') == 'qa_close_command' and type(row.get('ticket')) is int:
            stages.setdefault(row['ticket'], set()).add((row.get('stage'), row.get('detail')))
    return any({('queued', 'document'), ('deferred', 'document-busy')} <= seen for seen in stages.values())


def validate_observations(steps, records):
    def checkpoint(stage):
        values = [row['details'] for row in records if row.get('stage') == stage]
        if len(values) != 1 or not isinstance(values[0], dict):
            raise ValueError('Regression checkpoint missing/duplicate: ' + stage)
        return values[0]

    def menus(stage):
        value = checkpoint(stage)
        if value.get('window_enabled') is not True or value.get('disabled_top_level') != [] or value.get('exit_enabled') is not True:
            raise ValueError('Window or menus disabled after the save prompt: ' + stage)

    def panes(stage, text):
        value = checkpoint(stage).get('panes')
        if not isinstance(value, list) or [pane.get('text') for pane in value] != [text, text]:
            raise ValueError('Both panes must show the shared document: ' + stage)

    status = {row['id']: row['status'] for row in steps}
    if status.get('s1') == 'PASS':
        value = checkpoint('find focus announced')
        if (value.get('name') != 'Find' or value.get('focus') is not True or value.get('focused_is_field') is not True
                or value.get('text') != QUERY):
            raise ValueError('Find field focus or value was not observed')
        if checkpoint('find count announced').get('name') != fixture()['find_status']:
            raise ValueError('Find result count was not announced')
        if checkpoint('find escape editor focus').get('focus') is not True:
            raise ValueError('Escape did not return focus to the Editor')
    if status.get('s2') == 'PASS':
        for stage in ('close prompt cancel', 'close prompt discard'):
            if checkpoint(stage).get('owned') is not True:
                raise ValueError('Save prompt was not an owned editor dialog: ' + stage)
        menus('menus after cancel')
        menus('menus after discard')
        if checkpoint('untitled kept after cancel').get('text') != UNTITLED:
            raise ValueError('Cancel changed the dirty Untitled document')
        if checkpoint('untitled discarded').get('modified_tabs') != []:
            raise ValueError("Don't Save left a modified tab")
    if status.get('s3') == 'PASS':
        for stage, pane in [('pane 2 focused', 2), ('F6 to pane 1', 1), ('F6 to pane 2', 2), ('F6 back to pane 1', 1)]:
            if checkpoint(stage).get('focused_pane') != pane:
                raise ValueError('Split-pane focus differs: ' + stage)
        panes('pane 2 typed at its caret', 'A' + INITIAL)
        panes('pane 1 typed at its caret', fixture()['split_edited'])
        panes('split edits undone', INITIAL)
    if status.get('s4') == 'PASS':
        if not busy_close_deferred(checkpoint('busy close trace').get('records')):
            raise ValueError('Busy close was not deferred')
        if checkpoint('busy close completed').get('busy_tabs') != 0:
            raise ValueError('Busy document tab remained open')
        saved = checkpoint('busy saved prefix')
        if saved.get('prefix') != BUSY_EDIT + BUSY_LINE or saved.get('bytes') != BUSY_BYTES + len(BUSY_EDIT):
            raise ValueError('Busy save bytes differ')
    if status.get('s5') == 'PASS':
        for launch in range(1, RELAUNCHES + 1):
            window = checkpoint(f'relaunch {launch} window shown')
            # Found without a visibility filter (MainWindowHandle only reports shown windows).
            if (window.get('discovery') != 'any-visibility' or window.get('visible') is not True
                    or window.get('iconic') is not False):
                raise ValueError('Relaunched window was not shown')
            if checkpoint(f'relaunch {launch} restored text').get('text') != INITIAL:
                raise ValueError('Relaunch did not restore the document')
