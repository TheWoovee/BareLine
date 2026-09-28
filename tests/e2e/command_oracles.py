# SPDX-License-Identifier: MPL-2.0
"""Read and validate shared utility conversion regression fixtures."""
import hashlib
import json
from pathlib import Path

CATALOG = Path(__file__).with_name("utility_command_oracles.json")


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
