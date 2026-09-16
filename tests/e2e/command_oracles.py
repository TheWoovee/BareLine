# SPDX-License-Identifier: MPL-2.0
"""Concrete proposed command observations, with executable core support only."""
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CATALOG = Path(__file__).with_name('utility_command_oracles.json')


def catalog():
    raw = CATALOG.read_bytes()
    if len(raw) > 65536:
        raise ValueError('Command oracle catalogue exceeds its bound')
    document = json.loads(raw)
    if document.get('schema_version') != 1 or document.get('kind') != 'command_oracle_fixtures':
        raise ValueError('Unknown command oracle catalogue')
    rows = document['commands']
    if len({row['command'] for row in rows}) != len(rows):
        raise ValueError('Duplicate command oracle')
    for row in rows:
        if not row['success'] or not row['failures']:
            raise ValueError('Missing command vectors')
        cases = row['success'] + row['failures']
        if len({case['id'] for case in cases}) != len(cases):
            raise ValueError('Duplicate fixture identity')
        for case in cases:
            start, end = case['range']
            before = case['before'].encode('utf-8')
            if not 0 <= start <= end <= len(before) or case['max_bytes'] <= 0:
                raise ValueError('Invalid oracle range or budget')
            # Every declared byte boundary must split complete UTF-8 characters.
            for part in (before[:start], before[start:end], before[end:]):
                part.decode('utf-8')
    return document, hashlib.sha256(raw).hexdigest()


def bindings():
    document, digest = catalog()
    authority = [dict(path=path, sha256=hashlib.sha256((ROOT / path).read_bytes()).hexdigest())
                 for path in document['authority']]
    result = {}
    for row in document['commands']:
        common = dict(catalogue=str(CATALOG.relative_to(ROOT)), catalogue_sha256=digest,
                      authority=authority, core_test=row['core_test'],
                      core_test_scope='Supporting transaction checks only; not native command acceptance',
                      observation_status='NOT_RUN', review='pending', complete_case_mapping=False)
        for outcome in ('success', 'disabled', 'failure'):
            entry = dict(common, command=row['command'], outcome=outcome)
            entry['setup'] = ('Use a clean owned profile and UTF-8 document. Pin the runtime inventory and candidate. '
                              'Use exact fixture bytes and UTF-8 byte offsets; capture the initial file hash and clean state.')
            if outcome == 'success':
                entry['fixtures'] = row['success']
                entry['steps'] = [
                    'Run each success fixture separately. A whole-document fixture has no selection; otherwise select exactly the declared UTF-8 range.',
                    f'Invoke {row["command"]} through its actual registered user-facing route and wait for the worker to finish.',
                    'Compare the entire document with fixture.after; prefix/suffix outside the selection must remain byte-identical. The document becomes dirty and disk remains unchanged.',
                    'Undo once: exact fixture.before and clean state return. Redo once: exact fixture.after and dirty state return.',
                    'Save to the owned fixture path and compare its actual UTF-8 bytes with fixture.after. Retain observations of routing, transaction, Undo/Redo and disk output.',
                ]
            elif outcome == 'disabled':
                entry['fixture'] = row['success'][0]
                entry['steps'] = [
                    'Open the document fixture, enable File > Read-only, and record the saved-byte hash and clean state.',
                    f'Attempt {row["command"]} through its registered route. Capture the disabled state or explicit rejection; do not label missing UI as success.',
                    'If the route reaches utilities_dispatch, expect: The destination is read-only or has an edit pending. No document, disk, dirty-state or Undo mutation is allowed.',
                    'Clear read-only and repeat the success procedure to prove this fixture was blocked by context rather than an absent command.',
                ]
            else:
                entry['failure_fixture'] = row['native_failure']
                entry['core_negative_vectors'] = row['failures']
                entry['steps'] = [
                    'Materialize the declared native failure fixture. For a repeat recipe, use exactly count copies of repeat_utf8 and keep the document resident; for fixture_id, use that core-negative vector and its selection.',
                    f'Invoke {row["command"]} once and capture failure_fixture.expected_error exactly. Core-only budget arguments must not be passed off as native configuration.',
                    'Compare document bytes, saved-file hash, clean state and Undo history to the baseline: all remain unchanged and no partial output is inserted.',
                    'Correct the invalid input or reduce the encoded selection to the small success fixture, repeat the command and compare its exact expected output.',
                ]
            result[(row['command'], outcome)] = entry
    return result
