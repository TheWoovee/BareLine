# SPDX-License-Identifier: MPL-2.0
"""Portable byte-verifiable evidence retention; never self-certifies acceptance."""
from pathlib import Path
import argparse
from collections import Counter, deque
import os
import hashlib
import json
import ntpath
import posixpath
import re
import stat
import sys
import shutil
import tempfile
import uuid

from evidence_json import loads, read_bounded_bytes, read_json, MAX_JSON_BYTES

ROOT = Path(__file__).resolve().parents[2]
MAX_FILES = 10000
MAX_FILE_BYTES = 1024 * 1024 * 1024
MAX_TOTAL_BYTES = 4 * 1024 * 1024 * 1024
CHUNK_BYTES = 64 * 1024
KINDS = {'file', 'source', 'configuration', 'binary', 'screenshot', 't09_receipt', 'native_result',
         'evidence_bundle', 'environment_matrix', 'coverage_report', 'backlog', 'failure', 'review',
         'typed_result', 'typed_mapping', 'release_closure', 'readiness_record'}
SHA256 = re.compile(r'[0-9a-f]{64}')


def require(condition, message):
    if not condition:
        raise ValueError(message)


def safe_file(root, name):
    """Reject escaping paths and reparse ancestors before opening source data."""
    root = Path(os.path.abspath(root))
    for part in (root, *root.parents):
        info = part.lstat()
        require(not stat.S_ISLNK(info.st_mode) and not getattr(info, 'st_file_attributes', 0) & 0x400,
                'Reparse/symlink collection root refused')
    require(isinstance(name, str) and 0 < len(name) <= 4096 and '\0' not in name, 'Invalid evidence path')
    supplied = Path(name)
    require('..' not in supplied.parts, 'Parent traversal in evidence path')
    path = Path(os.path.abspath(root / supplied))
    require(path.is_relative_to(root) and path != root, 'Evidence path escapes collection root')
    for part in (path, *path.parents):
        info = part.lstat()
        require(not stat.S_ISLNK(info.st_mode) and not getattr(info, 'st_file_attributes', 0) & 0x400,
                'Reparse/symlink evidence path refused')
        if part == root:
            break
    require(stat.S_ISREG(path.stat().st_mode), 'Evidence must be a regular file')
    return path


def collection_plan(document, root=ROOT):
    require(isinstance(document, dict) and type(document.get('schema_version')) is int
            and document['schema_version'] == 1 and document.get('kind') == 'evidence_collection_plan'
            and document.keys() == {'schema_version', 'kind', 'label', 'files'}, 'Invalid evidence collection plan')
    require(isinstance(document['label'], str) and 0 < len(document['label'].strip()) <= 200, 'Collection label required')
    files = document['files']
    require(isinstance(files, list) and 0 < len(files) <= MAX_FILES, 'Invalid collection file count')
    selected = {}
    for row in files:
        require(isinstance(row, dict) and row.keys() == {'path', 'kind'} and row['kind'] in KINDS,
                'Invalid collection file declaration')
        path = safe_file(root, row['path'])
        require(path not in selected, 'Duplicate collection file')
        selected[path] = row['kind']
    return selected


def file_identity(info):
    # Windows path and handle stat results do not expose comparable ctime values.
    return (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns,
            info.st_ctime_ns if os.name != 'nt' else None)


def copy_object(source, objects, budget):
    """Hash and copy one held file with bounded buffers and cumulative quotas."""
    before = source.lstat()
    require(stat.S_ISREG(before.st_mode) and not getattr(before, 'st_file_attributes', 0) & 0x400,
            'Only regular source files may be copied')
    require(before.st_size <= MAX_FILE_BYTES, 'Evidence file exceeds byte limit')
    temporary = objects / ('.copy-' + uuid.uuid4().hex)
    size = 0
    digest = hashlib.sha256()
    try:
        with source.open('rb') as incoming, temporary.open('xb') as outgoing:
            require(file_identity(os.fstat(incoming.fileno())) == file_identity(before), 'Source changed before copy')
            while chunk := incoming.read(CHUNK_BYTES):
                size += len(chunk)
                require(size <= MAX_FILE_BYTES and budget['bytes'] + len(chunk) <= MAX_TOTAL_BYTES,
                        'Evidence copy exceeds byte limit')
                budget['bytes'] += len(chunk)
                digest.update(chunk)
                outgoing.write(chunk)
            require(file_identity(os.fstat(incoming.fileno())) == file_identity(before)
                    and file_identity(source.lstat()) == file_identity(before), 'Source changed during copy')
            outgoing.flush()
            os.fsync(outgoing.fileno())
        value = digest.hexdigest()
        destination = objects / value
        try:
            os.link(temporary, destination)
        except FileExistsError:
            with destination.open('rb') as existing:
                require(hashlib.file_digest(existing, 'sha256').hexdigest() == value, 'Conflicting retained object')
        return {'object': 'objects/' + value, 'sha256': value, 'bytes': size}
    finally:
        temporary.unlink(missing_ok=True)


def references(document, kind):
    """Known format references only; filenames in arbitrary prose are not paths."""
    links = []
    def add(path, digest, role):
        require(isinstance(path, str) and isinstance(digest, str) and SHA256.fullmatch(digest),
                'Dependency needs an exact path and SHA-256')
        links.append((path, digest, role))
    if kind == 't09_receipt':
        for stream in ('stdout', 'stderr'):
            record = document[stream]
            add(record['path'], record['sha256'], 'file')
    elif kind == 'native_result':
        request = document['request']
        add(request['executable'], request['binary_sha256'], 'binary')
        for step in document['steps']:
            for artifact in step.get('artifacts', []):
                add(artifact['path'], artifact['sha256'], 'file')
    elif kind == 'evidence_bundle':
        for record in document['evidence']:
            if record.get('excluded'):
                add(record['authority'], record['authority_sha256'], 'review')
                continue
            add(record['result'], record['result_sha256'], 'typed_result' if record.get('producer_kind') else 'native_result')
            add(record['test_receipt'], record['test_receipt_sha256'], 't09_receipt')
            if record.get('producer_kind'):
                add(record['mapping_source'], record['mapping_source_sha256'], 'typed_mapping')
            for fixture in record.get('fixtures', []):
                add(fixture['path'], fixture['sha256'], 'file')
    elif kind == 'environment_matrix':
        for row in document['exclusions']:
            add(row['authority'], row['authority_sha256'], 'review')
    elif kind == 'typed_result':
        add(document['test_receipt'], document['test_receipt_sha256'], 't09_receipt')
        add(document['procedure_file'], document['procedure_file_sha256'], 'file')
        for artifact in document['artifacts']:
            add(artifact['path'], artifact['sha256'], 'file')
    elif kind == 'release_closure':
        for key, role in [('prerequisite_report','coverage_report'),('prerequisite_receipt','t09_receipt'),('readiness_record','readiness_record')]:
            add(document[key]['path'], document[key]['sha256'], role)
    elif kind == 'readiness_record':
        for reference in document['performance_evidence']:
            add(reference['path'], reference['sha256'], 'typed_result')
    return links


STRUCTURED_KINDS = {'t09_receipt', 'native_result', 'evidence_bundle', 'environment_matrix', 'coverage_report', 'backlog',
                    'typed_result', 'typed_mapping', 'release_closure', 'readiness_record'}


def discover_dependencies(selected, root=ROOT):
    pending = deque((path, kind, None) for path, kind in selected.items())
    files, visited, edges = {}, set(), []
    while pending:
        supplied, kind, expected = pending.popleft()
        path = safe_file(root, str(supplied))
        record = files.setdefault(path, {'kinds': set(), 'expected': None})
        require(len(files) <= MAX_FILES, 'Dependency closure exceeds file limit')
        if expected:
            require(record['expected'] in (None, expected), 'Conflicting dependency hashes')
            record['expected'] = expected
        record['kinds'].add(kind)
        if (path, kind) in visited:
            continue
        visited.add((path, kind))
        if kind not in STRUCTURED_KINDS:
            continue
        raw = read_bounded_bytes(path, MAX_JSON_BYTES)
        actual = hashlib.sha256(raw).hexdigest()
        require(record['expected'] in (None, actual), 'Dependency bytes changed')
        record['expected'] = actual
        document = loads(raw)
        require(isinstance(document, dict), 'Structured evidence must be an object')
        for name, digest, role in references(document, kind):
            target = safe_file(root, name)
            edges.append({'from': str(path), 'to': str(target), 'sha256': digest, 'kind': role})
            require(len(edges) <= MAX_FILES * 10, 'Too many evidence dependencies')
            pending.append((target, role, digest))
    return files, edges


def write_new(path, raw):
    with path.open('xb') as stream:
        stream.write(raw)
        stream.flush()
        os.fsync(stream.fileno())


def encoded(document):
    return (json.dumps(document, ensure_ascii=False, sort_keys=True, indent=2, allow_nan=False) + '\n').encode('utf-8')


def remove_owned(directory, parent, name):
    # Recursive cleanup is limited to the exact directory created by this call.
    require(directory.name == name and directory.resolve().parent == parent.resolve()
            and directory.resolve() == parent.resolve() / name, 'Unsafe bundle cleanup target')
    shutil.rmtree(directory)


def collect(document, output, root=ROOT):
    root = Path(os.path.abspath(root))
    selected = collection_plan(document, root)
    files, edges = discover_dependencies(selected, root)
    destination = Path(os.path.abspath(output))
    require(not os.path.lexists(destination), 'Refusing to overwrite evidence bundle')
    parent = destination.parent
    parent.mkdir(parents=True, exist_ok=True)
    for part in (parent, *parent.parents):
        info = part.lstat()
        require(not stat.S_ISLNK(info.st_mode) and not getattr(info, 'st_file_attributes', 0) & 0x400,
                'Bundle destination has a reparse ancestor')
    stage = Path(tempfile.mkdtemp(prefix='.evidence-stage-', dir=parent))
    created = published = False
    try:
        objects = stage / 'objects'; objects.mkdir()
        budget, retained = {'bytes': 0}, []
        for path, details in sorted(files.items(), key=lambda item: str(item[0])):
            record = copy_object(safe_file(root, str(path)), objects, budget)
            require(details['expected'] in (None, record['sha256']), 'Copied dependency bytes changed')
            retained.append(dict(record, original=str(path), relative=path.relative_to(root).as_posix(),
                                 kinds=sorted(details['kinds'])))
        manifest = {'schema_version': 1, 'kind': 'retained_evidence_bundle', 'label': document['label'],
                    'source_root': str(root), 'roots': [{'original': str(path), 'kind': kind} for path, kind in selected.items()],
                    'files': retained, 'dependencies': edges, 'bytes_read': budget['bytes'],
                    'scope': 'Byte integrity and retained dependency closure; no semantic acceptance assertion'}
        raw = encoded(manifest)
        require(len(raw) <= MAX_JSON_BYTES, 'Bundle manifest exceeds JSON limit')
        digest = hashlib.sha256(raw).hexdigest()
        name = 'manifest-' + digest + '.json'
        write_new(stage / name, raw)
        write_new(stage / 'bundle.json', encoded({'schema_version': 1, 'manifest': name, 'sha256': digest}))
        destination.mkdir()  # Exclusive reservation; even an empty existing directory is refused.
        created = True
        os.rename(objects, destination / 'objects')
        os.link(stage / name, destination / name)
        os.link(stage / 'bundle.json', destination / 'bundle.json')  # Commit marker appears last.
        published = True
        return {'bundle': str(destination), 'manifest_sha256': digest, 'files': len(retained), 'bytes_read': budget['bytes']}
    finally:
        remove_owned(stage, parent, stage.name)
        if created and not published:
            remove_owned(destination, parent, destination.name)


def original_name(root, name):
    """Normalize retained identities using the original host's path grammar."""
    require(isinstance(root, str) and isinstance(name, str) and 0 < len(name) <= 4096
            and '\0' not in name, 'Invalid original evidence identity')
    grammar = ntpath if ntpath.splitdrive(root)[0] else posixpath
    require(grammar.isabs(root), 'Original collection root must be absolute')
    result = grammar.normcase(grammar.normpath(grammar.join(root, name)))
    base = grammar.normcase(grammar.normpath(root))
    require(grammar.commonpath((base, result)) == base and result != base, 'Original identity escapes collection root')
    return result


def verify(directory, expected_sha256=None):
    directory = Path(os.path.abspath(directory))
    for part in (directory, *directory.parents):
        info = part.lstat()
        require(not stat.S_ISLNK(info.st_mode) and not getattr(info, 'st_file_attributes', 0) & 0x400,
                'Reparse/symlink bundle directory refused')
    pointer = read_json(safe_file(directory, 'bundle.json'))
    require(isinstance(pointer, dict) and pointer.keys() == {'schema_version', 'manifest', 'sha256'}
            and type(pointer['schema_version']) is int and pointer['schema_version'] == 1
            and isinstance(pointer['sha256'], str) and SHA256.fullmatch(pointer['sha256']), 'Invalid bundle commit marker')
    digest = pointer['sha256']
    require(expected_sha256 is None or digest == expected_sha256, 'Bundle manifest differs from pinned hash')
    require(pointer['manifest'] == 'manifest-' + digest + '.json', 'Invalid manifest-addressed path')
    raw = read_bounded_bytes(safe_file(directory, pointer['manifest']), MAX_JSON_BYTES)
    require(hashlib.sha256(raw).hexdigest() == digest, 'Bundle manifest hash mismatch')
    manifest = loads(raw)
    require(isinstance(manifest, dict) and type(manifest.get('schema_version')) is int
            and manifest['schema_version'] == 1 and manifest.get('kind') == 'retained_evidence_bundle',
            'Unsupported retained evidence manifest')
    require(manifest.keys() == {'schema_version', 'kind', 'label', 'source_root', 'roots', 'files',
                                'dependencies', 'bytes_read', 'scope'}, 'Invalid retained manifest fields')
    files = manifest['files']
    require(isinstance(files, list) and 0 < len(files) <= MAX_FILES, 'Invalid retained file count')
    originals, checked, total = {}, {}, 0
    for record in files:
        require(isinstance(record, dict) and record.keys() == {'original', 'relative', 'kinds', 'object', 'sha256', 'bytes'},
                'Invalid retained file fields')
        require(isinstance(record['sha256'], str) and SHA256.fullmatch(record['sha256'])
                and type(record['bytes']) is int and 0 <= record['bytes'] <= MAX_FILE_BYTES,
                'Invalid retained file identity')
        require(record['object'] == 'objects/' + record['sha256'], 'Retained object path mismatch')
        name = original_name(manifest['source_root'], record['original'])
        require(name == original_name(manifest['source_root'], record['relative']) and name not in originals,
                'Duplicate or inconsistent retained original path')
        require(isinstance(record['kinds'], list) and record['kinds'] and all(kind in KINDS for kind in record['kinds'])
                and len(record['kinds']) == len(set(record['kinds'])), 'Invalid retained evidence kinds')
        originals[name] = record
        total += record['bytes']
        require(total <= MAX_TOTAL_BYTES, 'Retained bundle exceeds total byte limit')
        if record['sha256'] not in checked:
            path = safe_file(directory, record['object'])
            actual = hashlib.sha256(); size = 0
            with path.open('rb') as stream:
                while chunk := stream.read(CHUNK_BYTES):
                    size += len(chunk)
                    require(size <= record['bytes'], 'Retained object size mismatch')
                    actual.update(chunk)
            require(size == record['bytes'] and actual.hexdigest() == record['sha256'], 'Retained object hash/size mismatch')
            checked[record['sha256']] = size
        require(checked[record['sha256']] == record['bytes'], 'Conflicting retained object size')
    require(type(manifest.get('bytes_read')) is int and manifest['bytes_read'] == total, 'Bundle byte accounting mismatch')
    require(isinstance(manifest['dependencies'], list) and len(manifest['dependencies']) <= MAX_FILES * 10,
            'Invalid retained dependency count')
    actual_edges, declared_edges = Counter(), Counter()
    adjacency = {}
    for edge in manifest['dependencies']:
        require(isinstance(edge, dict) and edge.keys() == {'from', 'to', 'sha256', 'kind'}, 'Invalid dependency fields')
        source = original_name(manifest['source_root'], edge['from'])
        target = original_name(manifest['source_root'], edge['to'])
        require(source in originals and target in originals and originals[target]['sha256'] == edge['sha256']
                and edge['kind'] in originals[target]['kinds'], 'Retained dependency missing or mismatched')
        declared_edges[(source, target, edge['sha256'], edge['kind'])] += 1
        adjacency.setdefault(source, set()).add(target)
    for name, record in originals.items():
        for kind in record['kinds']:
            if kind not in STRUCTURED_KINDS:
                continue
            raw = read_bounded_bytes(safe_file(directory, record['object']), MAX_JSON_BYTES)
            require(hashlib.sha256(raw).hexdigest() == record['sha256'], 'Retained structured object changed')
            document = loads(raw)
            require(isinstance(document, dict), 'Structured evidence must be an object')
            for path, value, role in references(document, kind):
                actual_edges[(name, original_name(manifest['source_root'], path), value, role)] += 1
    require(actual_edges == declared_edges, 'Retained dependency closure is incomplete or rewritten')
    roots = manifest['roots']
    require(isinstance(roots, list) and 0 < len(roots) <= MAX_FILES, 'Invalid retained collection roots')
    pending, root_names = [], set()
    for row in roots:
        require(isinstance(row, dict) and row.keys() == {'original', 'kind'}, 'Invalid retained root fields')
        name = original_name(manifest['source_root'], row['original'])
        require(name in originals and row['kind'] in originals[name]['kinds'] and name not in root_names,
                'Missing or duplicate retained root')
        pending.append(name); root_names.add(name)
    reachable = set()
    while pending:
        name = pending.pop()
        if name in reachable: continue
        reachable.add(name); pending.extend(adjacency.get(name, ()))
    require(reachable == originals.keys(), 'Retained manifest contains unreachable files')
    expected = {'objects', 'bundle.json', pointer['manifest']}
    for entry in directory.iterdir():
        require(entry.name in expected, 'Unexpected file in retained bundle')
    for entry in (directory / 'objects').iterdir():
        require(entry.name in checked, 'Unexpected retained object')
    return {'integrity_verified': True, 'semantic_acceptance': False, 'manifest_pinned': expected_sha256 is not None,
            'manifest_sha256': digest, 'file_count': len(files), 'unique_objects': len(checked),
            'bytes': total, 'manifest': manifest}


def verify_producers(directory, expected_sha256=None):
    """Recompute supported producer observations using retained objects only."""
    import typed_producers
    checked = verify(directory, expected_sha256)
    manifest = checked['manifest']
    objects = {record['original']:safe_file(directory, record['object']) for record in manifest['files']}
    results, evidence, native = [], [], []
    with typed_producers.retained_reader(manifest['source_root'], objects):
        for record in manifest['files']:
            roles = set(record['kinds'])
            if 'native_result' in roles: native.append(record['original'])
            if 'typed_result' in roles:
                result = typed_producers.checked_result(objects[record['original']])
                results.append({'original':record['original'],'sha256':record['sha256'],
                    'producer_kind':result['producer_kind'],'status':result['status'],'checks':result['checks']})
            if 'evidence_bundle' in roles:
                data = read_json(objects[record['original']])
                for item in data['evidence']:
                    if item.get('producer_kind'):
                        typed_producers.validate_record(item)
                        evidence.append({'id':item['id'],'cell_id':item['cell_id'],'status':item['status']})
    return {'schema_version':1,'kind':'retained_producer_verification','manifest_sha256':checked['manifest_sha256'],
        'typed_result_count':len(results),'typed_evidence_count':len(evidence),'results':results,'evidence':evidence,
        'native_results_not_recomputed':native,'semantic_acceptance':False,
        'scope':'Recomputed supported producer observations and mappings. Final matrix resolution, native acceptance and independent release review remain separate.'}

def readiness_report(directory, expected_sha256=None):
    checked = verify(directory, expected_sha256)
    work, coverage, negatives, inputs = {}, [], [], []
    for record in checked['manifest']['files']:
        roles = set(record['kinds'])
        if not roles & {'backlog', 'coverage_report', 'native_result', 'typed_result', 't09_receipt'}:
            continue
        raw = read_bounded_bytes(safe_file(directory, record['object']), MAX_JSON_BYTES)
        require(hashlib.sha256(raw).hexdigest() == record['sha256'], 'Report input bytes changed')
        data = loads(raw)
        inputs.append({'original': record['original'], 'sha256': record['sha256'], 'kinds': record['kinds']})
        if 'backlog' in roles:
            require(type(data.get('schema_version')) is int and data['schema_version'] == 1
                    and isinstance(data.get('items'), list) and len(data['items']) <= 10000, 'Invalid retained backlog')
            for item in data['items']:
                require(isinstance(item, dict) and all(isinstance(item.get(key), str) and item[key].strip()
                        for key in ('id', 'title', 'kind', 'status')), 'Incomplete backlog work item')
                require(item['id'] not in work, 'Conflicting retained backlog work item')
                work[item['id']] = {key: item[key] for key in ('id', 'title', 'kind', 'status')}
        if 'coverage_report' in roles:
            statuses = {'PASS', 'FAIL', 'NOT_RUN', 'MISSING', 'EXCLUDED'}
            require(type(data.get('schema_version')) is int and data['schema_version'] == 1
                    and type(data.get('evidence_complete')) is bool and isinstance(data.get('resolved'), dict)
                    and len(data['resolved']) <= 40000 and isinstance(data.get('unresolved'), list), 'Invalid retained coverage report')
            require(all(isinstance(key, str) and status in statuses for key, status in data['resolved'].items()),
                    'Invalid retained case status')
            rows = data.get('cell_coverage', [])
            require(isinstance(rows, list) and len(rows) <= MAX_FILES * 10, 'Invalid retained cell coverage')
            unique_cells = set()
            for row in rows:
                require(isinstance(row, dict) and row.keys() == {'id', 'cell_id', 'status'}
                        and isinstance(row['id'], str) and isinstance(row['cell_id'], str)
                        and row['status'] in statuses, 'Invalid retained cell status')
                key = (row['id'], row['cell_id'])
                require(key not in unique_cells, 'Duplicate retained coverage cell')
                unique_cells.add(key)
            if data['evidence_complete']:
                require(data['resolved'] and not data['unresolved']
                        and all(status in {'PASS', 'EXCLUDED'} for status in data['resolved'].values())
                        and all(row['status'] in {'PASS', 'EXCLUDED'} for row in rows),
                        'Coverage completion contradicts retained failures')
            coverage.append({'original': record['original'], 'sha256': record['sha256'],
                             'reported_complete': data['evidence_complete'], 'cases': data['resolved'],
                             'cells': rows, 'unresolved': data['unresolved']})
        if (roles & {'native_result', 'typed_result'} and data.get('status') in {'FAIL', 'NOT_RUN'}) or (
                't09_receipt' in roles and type(data.get('exit_code')) is int and data['exit_code'] != 0):
            negatives.append({'original': record['original'], 'sha256': record['sha256'],
                              'status': data.get('status'), 'exit_code': data.get('exit_code')})
    completed = [item for item in work.values() if item['status'] in {'implemented-focused-verified', 'closed-dispositioned'}]
    deferred = [item for item in work.values() if item['status'] == 'deferred-owner-next-update']
    remaining = [item for item in work.values() if item not in completed and item not in deferred]
    missing = []
    if not work: missing.append('No retained implementation backlog')
    if not coverage: missing.append('No retained case/environment coverage report')
    return {'schema_version': 1, 'kind': 'retained_readiness_report', 'manifest_sha256': checked['manifest_sha256'],
            'integrity_verified': True, 'manifest_pinned': checked['manifest_pinned'], 'semantic_acceptance': False,
            'work_item_count': len(work), 'remaining_count': len(remaining),
            'completed_count': len(completed), 'deferred_count': len(deferred),
            'deferred_work': sorted(deferred, key=lambda item: item['id']),
            'remaining_by_kind': dict(sorted(Counter(item['kind'] for item in remaining).items())),
            'remaining_work': sorted(remaining, key=lambda item: item['id']),
            'open_issues_and_decisions': [item for item in remaining if item['status'] in {'confirmed-integration-gap', 'decision-required'}],
            'coverage': coverage, 'retained_negative_captures': negatives, 'inputs': inputs, 'missing_report_inputs': missing,
            'release_approval': 'not_attested', 'signing_and_reproducibility': 'not_attested',
            'scope': 'Retained status reporting and byte verification. Synthetic tests, reviewer labels and these counts do not establish product acceptance.'}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='operation', required=True)
    capture = commands.add_parser('collect')
    capture.add_argument('--plan', type=Path, required=True)
    capture.add_argument('--source-root', type=Path, default=ROOT)
    capture.add_argument('--output', type=Path, required=True)
    for name in ('verify', 'verify-producers', 'report'):
        operation = commands.add_parser(name)
        operation.add_argument('--bundle', type=Path, required=True)
        operation.add_argument('--expected-sha256', help='Manifest hash retained independently at collection')
        operation.add_argument('--output', type=Path)
    args = parser.parse_args(argv)
    try:
        if args.operation == 'collect':
            result = collect(read_json(args.plan), args.output, args.source_root)
        else:
            if args.output:
                require(not args.output.resolve().is_relative_to(args.bundle.resolve()), 'Report output cannot mutate its immutable bundle')
            if args.operation == 'verify':
                result = verify(args.bundle, args.expected_sha256)
                result.pop('manifest')
            elif args.operation == 'verify-producers':
                result = verify_producers(args.bundle, args.expected_sha256)
            else:
                result = readiness_report(args.bundle, args.expected_sha256)
            if args.output:
                write_new(args.output, encoded(result))
        print(json.dumps(result, ensure_ascii=False, sort_keys=True))
        if args.operation == 'report':
            return int(bool(result['missing_report_inputs'] or result['remaining_count']
                            or any(not row['reported_complete'] for row in result['coverage'])))
        return 0
    except (ValueError, KeyError, TypeError, OSError) as error:
        print(str(error), file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
