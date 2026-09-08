# SPDX-License-Identifier: MPL-2.0
"""Create an explicit bounded-storage ASCII fixture and immutable hash receipt."""
import argparse
import hashlib
from pathlib import Path
import shutil
from perf_suite import write_new


def generate(path, length, long_line):
    path = Path(path).resolve()
    if not 1 <= length <= 5 * 1024 * 1024 * 1024:
        raise ValueError('fixture length must be 1 byte..5 GiB')
    if shutil.disk_usage(path.parent).free < length + 64 * 1024 * 1024:
        raise ValueError('insufficient fixture disk headroom')
    prefix = b'// PERF_NEEDLE generated fixture '
    line = prefix + b'x' * (127 - len(prefix)) + b'\n'
    block = b'x' * 65536 if long_line else line * 512
    value = hashlib.sha256()
    with path.open('xb') as output:
        remaining = length
        if long_line:
            initial = prefix[:remaining]
            output.write(initial); value.update(initial); remaining -= len(initial)
        while remaining:
            part = block[:min(remaining, len(block))]
            output.write(part); value.update(part); remaining -= len(part)
    receipt = {'schema_version': 1, 'path': str(path), 'sha256': value.hexdigest(),
               'disk_bytes': length, 'expected_text_bytes': length, 'encoding': 'UTF-8 ASCII no BOM',
               'line_bytes': None if long_line else 128, 'long_line': long_line,
               'result_jump_token': 'PERF_NEEDLE', 'absent_search_token': 'PERF_ABSENT_TOKEN',
               'cache_state': 'generation warms file cache; not a cold-open fixture'}
    write_new(str(path) + '.fixture.json', receipt)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('path')
    parser.add_argument('--bytes', type=int, required=True)
    parser.add_argument('--long-line', action='store_true')
    args = parser.parse_args()
    generate(args.path, args.bytes, args.long_line)


if __name__ == '__main__':
    main()
