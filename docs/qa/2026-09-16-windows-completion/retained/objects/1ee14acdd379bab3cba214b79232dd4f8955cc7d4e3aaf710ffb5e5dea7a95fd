# SPDX-License-Identifier: MPL-2.0
"""Reviewed environment requirements for native evidence; no execution claims."""
import re

ENVIRONMENT_FIELDS = frozenset(('os_family', 'os_build', 'architecture', 'hardware', 'mode',
                                'theme', 'dpi', 'renderer', 'assistive_technology', 'build_mode'))
CELL_ID = re.compile(r'[a-z0-9][a-z0-9_.-]{0,79}')
SHA256 = re.compile(r'[0-9a-f]{64}')
MAX_REQUIRED_CELLS = 40000


def require(condition, message):
    if not condition:
        raise ValueError(message)


def environment(value):
    require(isinstance(value, dict) and value.keys() == ENVIRONMENT_FIELDS,
            'Environment must contain exactly the reviewed identity fields')
    require(all(isinstance(item, str) and item.strip() == item and 0 < len(item) <= 512
                and not any(ord(c) < 32 for c in item) for item in value.values()),
            'Environment values must be bounded nonempty strings')
    choices = {'os_family': {'windows', 'linux', 'macos'}, 'architecture': {'x64', 'arm64'},
               'mode': {'keyboard', 'pointer', 'screen_reader', 'headless'},
               'theme': {'dark', 'light', 'high_contrast', 'not_applicable'},
               'dpi': {'100', '125', '150', '200', '250', '300', 'not_applicable'},
               'renderer': {'software', 'hardware', 'not_applicable'},
               'build_mode': {'preview', 'configured', 'fixture'}}
    for key, allowed in choices.items():
        require(value[key] in allowed, 'Unsupported environment ' + key)
    require(value['mode'] != 'screen_reader' or value['assistive_technology'] != 'none',
            'Screen-reader environment must identify the assistive technology/version')
    return dict(value)


def source_identity(value):
    require(isinstance(value, dict) and value.get('available') is True
            and isinstance(value.get('head'), str) and re.fullmatch(r'[0-9a-f]{40}', value['head'])
            and type(value.get('working_tree_dirty')) is bool
            and isinstance(value.get('source_manifest_sha256'), str)
            and SHA256.fullmatch(value['source_manifest_sha256']), 'Invalid cell source identity')
    return {key: value[key] for key in ('available', 'head', 'working_tree_dirty', 'source_manifest_sha256')}


def matrix(document, required_ids):
    require(isinstance(document, dict) and type(document.get('schema_version')) is int
            and document['schema_version'] in {1, 2} and document.get('kind') == 'qualification_environment_matrix',
            'Unsupported environment matrix')
    require(document.keys() == {'schema_version', 'kind', 'review', 'cells', 'requirements', 'exclusions'},
            'Unexpected or missing matrix fields')
    review = document['review']
    require(isinstance(review, dict) and review.keys() == {'implementer', 'reviewer'}
            and all(isinstance(v, str) and v.strip() for v in review.values()), 'Matrix review identity missing')
    rows = document['cells']
    require(isinstance(rows, list) and 0 < len(rows) <= 1000, 'Invalid environment cell count')
    named = {}
    for row in rows:
        native_fields = {'id', 'environment', 'source_identity', 'binary_sha256'}
        typed_fields = {'id', 'environment', 'source_identity', 'artifact_set_sha256', 'producer_kind'}
        require(isinstance(row, dict) and (row.keys() == native_fields or
                (document['schema_version'] == 2 and row.keys() == typed_fields)),
                'Invalid cell fields')
        key = row['id']
        require(isinstance(key, str) and CELL_ID.fullmatch(key) and key not in named, 'Invalid/duplicate cell ID')
        environment(row['environment'])
        source_identity(row['source_identity'])
        if 'binary_sha256' in row:
            require(isinstance(row['binary_sha256'], str) and SHA256.fullmatch(row['binary_sha256']), 'Invalid cell binary hash')
        else:
            require(isinstance(row['artifact_set_sha256'], str) and SHA256.fullmatch(row['artifact_set_sha256']), 'Invalid cell artifact set')
            require(row['producer_kind'] in {'rust_tests','python_tests','packaging','performance','manual','host_contract'}, 'Invalid cell producer kind')
        named[key] = row
    requirements = document['requirements']
    require(isinstance(requirements, dict) and 0 < len(requirements) <= 40000
            and requirements.keys() == set(required_ids), 'Matrix must cover every required case exactly')
    for key, names in requirements.items():
        require(isinstance(names, list) and 0 < len(names) <= 1000
                and all(isinstance(name, str) and name in named for name in names)
                and len(names) == len(set(names)), 'Invalid required cells for ' + key)
    require(sum(len(names) for names in requirements.values()) <= MAX_REQUIRED_CELLS, 'Too many required case/environment cells')
    require(isinstance(document['exclusions'], list) and len(document['exclusions']) <= 40000, 'Invalid cell exclusions')
    excluded = set()
    for row in document['exclusions']:
        require(isinstance(row, dict) and row.keys() == {'id', 'cell_id', 'reason', 'authority', 'authority_sha256'},
                'Invalid cell exclusion fields')
        require(isinstance(row['id'], str) and row['id'] in requirements
                and row['cell_id'] in requirements[row['id']], 'Exclusion targets an unknown required cell')
        key = (row['id'], row['cell_id'])
        require(key not in excluded, 'Duplicate cell exclusion')
        excluded.add(key)
        require(all(isinstance(row[field], str) and 0 < len(row[field]) <= 4096 and row[field].strip()
                    for field in ('reason', 'authority', 'authority_sha256'))
                and SHA256.fullmatch(row['authority_sha256']), 'Incomplete cell exclusion authority')
    return document


def index_records(records, document):
    """Uniqueness is case plus required cell; historical retries live in bundles."""
    require(isinstance(records, list) and len(records) <= 40000, 'Too many cell evidence records')
    indexed = {}
    for record in records:
        require(isinstance(record, dict), 'Invalid cell evidence record')
        case, cell = record.get('id'), record.get('cell_id')
        require(isinstance(case, str) and case in document['requirements'], 'Unknown cell evidence case')
        require(isinstance(cell, str) and cell in document['requirements'][case], 'Unknown or missing required cell')
        key = (case, cell)
        require(key not in indexed, 'Duplicate evidence for the same case and cell')
        indexed[key] = record
    return indexed


def require_match(record, cell):
    captured = environment({key: record.get(key) for key in ENVIRONMENT_FIELDS})
    require(captured == cell['environment'], 'Evidence environment differs from required cell')
    require(source_identity(record.get('source_identity')) == source_identity(cell['source_identity']),
            'Evidence source differs from required cell')
    if 'binary_sha256' in cell:
        require('producer_kind' not in record and record.get('binary_sha256') == cell['binary_sha256'], 'Evidence binary differs from required cell')
    else:
        require(record.get('artifact_set_sha256') == cell['artifact_set_sha256']
                and record.get('producer_kind') == cell['producer_kind'], 'Evidence artifacts/producer differ from required cell')


def require_exclusion(record, document):
    approved = [row for row in document['exclusions']
                if (row['id'], row['cell_id']) == (record['id'], record['cell_id'])]
    require(record.get('excluded') is True and len(approved) == 1
            and all(record.get(key) == value for key, value in approved[0].items()),
            'Cell exclusion differs from reviewed matrix authority')


def coverage(document, statuses):
    resolved, rows, unresolved = {}, [], []
    for case, names in sorted(document['requirements'].items()):
        outcomes = []
        for name in names:
            status = statuses.get((case, name), 'MISSING')
            require(status in {'PASS', 'FAIL', 'NOT_RUN', 'MISSING', 'EXCLUDED'}, 'Invalid cell coverage status')
            rows.append({'id': case, 'cell_id': name, 'status': status})
            outcomes.append(status)
            if status not in {'PASS', 'EXCLUDED'}:
                unresolved.append(f'{case}@{name}: {status}')
        if all(status in {'PASS', 'EXCLUDED'} for status in outcomes):
            resolved[case] = 'EXCLUDED' if all(status == 'EXCLUDED' for status in outcomes) else 'PASS'
        else:
            resolved[case] = next(status for status in ('FAIL', 'MISSING', 'NOT_RUN') if status in outcomes)
    return resolved, rows, unresolved
