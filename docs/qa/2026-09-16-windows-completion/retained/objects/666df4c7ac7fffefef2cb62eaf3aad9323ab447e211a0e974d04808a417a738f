# SPDX-License-Identifier: MPL-2.0
"""JSON evidence must have one unambiguous value for every field."""
import json
from pathlib import Path

MAX_JSON_BYTES = 16 * 1024 * 1024


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError('Duplicate JSON field: ' + key)
        result[key] = value
    return result


def loads(raw):
    if isinstance(raw, bytes):
        raw = raw.decode('utf-8-sig')
    return json.loads(raw, object_pairs_hook=unique_object)


def read_bounded_bytes(path, limit):
    # Bound the actual read, including a file that grows after it was inspected.
    with Path(path).open('rb') as source:
        raw = source.read(limit + 1)
    if len(raw) > limit:
        raise ValueError(f'Evidence input exceeds {limit} bytes')
    return raw


def read_json(path):
    return loads(read_bounded_bytes(path, MAX_JSON_BYTES))
