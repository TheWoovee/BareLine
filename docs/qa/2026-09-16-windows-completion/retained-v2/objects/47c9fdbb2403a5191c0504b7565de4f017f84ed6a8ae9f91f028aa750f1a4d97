# SPDX-License-Identifier: MPL-2.0
"""Synthetic oracle tests; these are not records of a native product run."""
import base64
import copy
import hashlib
from pathlib import Path
import tempfile
import tomllib
import unittest
import column_fixture as column
import split_fixture as split
import workspace_fixture as workspace
import udl_fixture as udl
import huge_log_fixture as log
import portable_fixture as portable
import macro_fixture as macro


def steps(*passed):
    return [dict(id=f's{i}', status='PASS' if i in passed else 'NOT_RUN') for i in (1, 2, 3)]


def records(values):
    return [dict(stage=stage, details=value) for stage, value in values.items()]


def saved(text):
    raw = text.encode()
    return dict(base64=base64.b64encode(raw).decode(), bytes=len(raw), sha256=hashlib.sha256(raw).hexdigest())


class WindowsFixtures(unittest.TestCase):
    def test_log_boundary_recipe_and_rotated_prefix_tamper(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'log'
            size = 2 * len(log.BLOCK); offset = len(log.BLOCK) - 4
            generated = log.generate(path, size, offset)
            raw = path.read_bytes()
            self.assertEqual(raw[offset:offset+len(log.NEEDLE.encode())], log.NEEDLE.encode())
            with path.open('ab') as stream: stream.write(log.APPEND.encode())
            log.verify_rotated(path, generated)
            with path.open('r+b') as stream: stream.write(b'!')
            with self.assertRaises(ValueError): log.verify_rotated(path, generated)

    def test_portable_requires_unchanged_move_and_exact_encoding(self):
        values = {'portable settings edited': saved(portable.fixture('dark')['settings']),
                  'portable containment before': dict(external_profile_files=[]),
                  'portable relocation': dict(old_exists=False, inventory_before=['one'], inventory_after=['one']),
                  'owned exit 1': dict(exit_code=0)}
        portable.validate_observations(steps(1, 2), records(values), 'dark')
        values['portable relocation']['old_exists'] = True
        with self.assertRaises(ValueError): portable.validate_observations(steps(1, 2), records(values), 'dark')

    def test_external_definition_retains_literal_arguments_and_macro_search(self):
        root = Path('C:/owned scratch')
        parsed = tomllib.loads(macro.external_definition(root))
        self.assertEqual(parsed['args'][-len(macro.ARGS):], macro.ARGS)
        raw = b'format_version=1\n[[events]]\ncommand="edit.insert_text"\ntext="REC "\n[[events]]\ncommand="search.find_next"\n[events.arguments]\npattern="target"\nmode="literal"\n[[events]]\ncommand="edit.insert_text"\ntext="FOUND"\n'
        macro.validate_macro(raw)
        with self.assertRaises(ValueError): macro.validate_macro(raw.replace(b'"target"', b'"wrong"'))

    def test_column_exact_selections_and_one_undo(self):
        values = {
            'column opened': dict(text=column.INITIAL),
            'column selections': dict(text=column.INITIAL, selections=[dict(start_utf16=n, text='') for n in column.POSITIONS]),
            'column original disk': saved(column.INITIAL), 'column insertion input': dict(text='|'),
            'column inserted': dict(text=column.INSERTED), 'column unsaved disk': saved(column.INITIAL),
            'column inserted disk': saved(column.INSERTED), 'column one Undo': dict(text=column.INITIAL),
            'column Undo disk unchanged': saved(column.INSERTED), 'column restored disk': saved(column.INITIAL),
        }
        column.validate_observations(steps(1, 2, 3), records(values))
        for stage, mutation in [('column selections', dict(text=column.INITIAL, selections=[dict(start_utf16=2, text='')])),
                                ('column inserted', dict(text=column.INSERTED.replace('x   |', 'x|'))),
                                ('column one Undo', dict(text=column.INITIAL + '|'))]:
            changed = copy.deepcopy(values);changed[stage] = mutation
            with self.assertRaises(ValueError): column.validate_observations(steps(1, 2, 3), records(changed))

    def test_split_requires_both_panes_motion_and_unchanged_focus(self):
        def pair(text, visible='row 000\n', focus=True):
            return dict(panes=[dict(name=f'Pane {n}, split.txt', text=text, runtime_id=[n], visible=visible,
                                   focus=focus if n == 1 else not focus) for n in (1, 2)])
        values = {stage: pair(text) for stage, text in [('split cloned', split.INITIAL),
            ('split primary edit', split.fixture()['first']), ('split primary Undo', split.INITIAL),
            ('split secondary edit', split.fixture()['second']), ('split secondary Undo', split.INITIAL)]}
        values.update({'split scroll before': pair(split.INITIAL),
                       'split scroll after': pair(split.INITIAL, 'row 045\n'), 'split sync menu': dict(checked=True)})
        split.validate_observations(steps(1, 2, 3), records(values))
        for key, bad in [('text', 'stale'), ('visible', ''), ('focus', True), ('runtime_id', [1])]:
            changed = copy.deepcopy(values);changed['split scroll after']['panes'][1][key] = bad
            with self.assertRaises(ValueError): split.validate_observations(steps(1, 2, 3), records(changed))

    def test_workspace_requires_exact_tree_and_outline_offset_after_rename(self):
        values = {'workspace tree': dict(names=['main.rs', 'notes.txt', 'workspace-fixture']),
                  'workspace target': dict(text=workspace.INITIAL, caret_utf16=workspace.fixture()['target_offset']),
                  'workspace renamed tree': dict(names=['notes.txt', 'renamed.rs', 'workspace-fixture']),
                  'workspace renamed target': dict(text=workspace.INITIAL, caret_utf16=workspace.fixture()['target_offset']),
                  'workspace rename paths': dict(old_exists=False)}
        workspace.validate_observations(steps(1, 2, 3), records(values))
        for stage in values:
            changed = copy.deepcopy(values);del changed[stage]
            with self.assertRaises(ValueError): workspace.validate_observations(steps(1, 2, 3), records(changed))
        values['workspace renamed target']['caret_utf16'] += 1
        with self.assertRaises(ValueError): workspace.validate_observations(steps(1, 2, 3), records(values))

    def test_udl_requires_installed_definition_and_real_restart_receipt(self):
        fixture = udl.fixture('dark')
        probes = [dict(kind=p['kind'], text=p['text'], bounds=[[0, 0, 4, 4]], histogram=[dict(rgb=p['rgb'], count=8)]) for p in fixture['probes']]
        values = {'udl import report': dict(name=fixture['import_status']),
                  'udl installed definition': dict(id='qa-fixture', keywords=['sentinel'], extensions=['qaudl']),
                  'udl sample': dict(text=udl.SAMPLE), 'udl restarted sample': dict(text=udl.SAMPLE),
                  'code highlighting': dict(probes=probes), 'udl restarted highlighting': dict(probes=probes),
                  'owned exit 1': dict(exit_code=0), 'owned launch 1': dict(pid=123), 'owned launch 2': dict(pid=456)}
        udl.validate_observations(steps(1, 2, 3), records(values), 'dark')
        values['owned launch 2']['pid'] = 123
        with self.assertRaises(ValueError): udl.validate_observations(steps(1, 2, 3), records(values), 'dark')


if __name__ == '__main__': unittest.main()
