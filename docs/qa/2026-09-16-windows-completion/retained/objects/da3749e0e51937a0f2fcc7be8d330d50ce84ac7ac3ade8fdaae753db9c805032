# SPDX-License-Identifier: MPL-2.0
"""Checked non-native producers. Results are observations until mapped/reviewed."""
from __future__ import annotations
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import sys
import tempfile
import contextlib
import contextvars
import ntpath
import posixpath

from evidence_json import read_json, loads, read_bounded_bytes
import qualification_cells as cells

ROOT = Path(__file__).resolve().parents[2]
KINDS = {'rust_tests', 'python_tests', 'packaging', 'performance', 'manual', 'host_contract'}
ARTIFACT_KINDS = {'source', 'windows_executable', 'host_executable', 'package', 'configuration', 'report', 'fixture', 'screenshot', 'authority'}
ID = re.compile(r'[a-zA-Z0-9_.:-]{1,200}')
SHA = re.compile(r'[a-f0-9]{64}')
RETAINED = contextvars.ContextVar('typed_retained_reader', default=None)

def namespace():
    context = RETAINED.get()
    return context['grammar'] if context else (ntpath if os.name == 'nt' else posixpath)

def basename(path):
    return namespace().basename(str(path))

def reference(path):
    context = RETAINED.get()
    root = context['root'] if context else str(ROOT)
    grammar = namespace()
    value = str(path)
    if not grammar.isabs(value): value = grammar.join(root, value)
    return grammar.normcase(grammar.normpath(value))

def repo_file(relative):
    context = RETAINED.get()
    return namespace().join(context['root'] if context else str(ROOT), relative)

@contextlib.contextmanager
def retained_reader(source_root, objects):
    grammar = ntpath if ntpath.splitdrive(source_root)[0] else posixpath
    normalized = {grammar.normcase(grammar.normpath(name)):Path(path) for name,path in objects.items()}
    require(len(normalized) == len(objects), 'ambiguous retained original paths')
    token = RETAINED.set({'root':source_root,'grammar':grammar,'objects':normalized,'physical':set(normalized.values())})
    try: yield
    finally: RETAINED.reset(token)

def require(condition, message):
    if not condition:
        raise ValueError(message)

def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=True).encode()

def digest(path):
    value = hashlib.sha256()
    with Path(path).open('rb') as stream:
        while block := stream.read(65536):
            value.update(block)
    return value.hexdigest()

def local(path):
    context = RETAINED.get()
    if context:
        if Path(path) in context['physical']: return Path(path)
        result = context['objects'].get(reference(path))
        require(result is not None, 'required artifact absent from retained bundle: '+str(path))
        return result
    raw = Path(path)
    path = raw if raw.is_absolute() else ROOT/raw
    for ancestor in (path, *path.parents):
        require(not ancestor.is_symlink() and not ancestor.is_junction(), 'reparse evidence path')
    path = path.resolve()
    require(path.is_relative_to(ROOT), 'typed evidence path escapes repository')
    return path

def artifact_set(records):
    require(isinstance(records, list) and 0 < len(records) <= 4096, 'bounded nonempty artifact set required')
    roles = set()
    identities = []
    for item in records:
        require(isinstance(item, dict) and item.keys() == {'role', 'kind', 'path', 'sha256', 'bytes'}, 'invalid typed artifact fields')
        require(isinstance(item['role'], str) and ID.fullmatch(item['role']) and item['role'] not in roles, 'invalid/duplicate artifact role')
        roles.add(item['role'])
        require(item['kind'] in ARTIFACT_KINDS and isinstance(item['path'], str) and item['path'], 'invalid artifact kind/path')
        require(isinstance(item['sha256'], str) and SHA.fullmatch(item['sha256']) and type(item['bytes']) is int and 0 <= item['bytes'] <= 1024**3, 'invalid artifact identity')
        identities.append({key: item[key] for key in ('role','kind','sha256','bytes')})
    require(sum(item['bytes'] for item in records) <= 4*1024**3, 'artifact set exceeds retention budget')
    return hashlib.sha256(canonical(sorted(identities, key=lambda item: item['role']))).hexdigest()

def procedure(value):
    require(isinstance(value, dict) and value.keys() == {'schema_version','id','producer_kind','command','checks','artifacts','environment','timeout_seconds'}, 'invalid typed procedure fields')
    require(type(value['schema_version']) is int and value['schema_version'] == 1 and ID.fullmatch(value['id']), 'invalid typed procedure schema/id')
    require(value['producer_kind'] in KINDS, 'unsupported typed producer')
    command = value['command']
    require(isinstance(command, list) and 1 <= len(command) <= 128 and all(isinstance(part, str) and part and '\0' not in part for part in command), 'invalid producer argv')
    require(type(value['timeout_seconds']) is int and 1 <= value['timeout_seconds'] <= 86400, 'invalid producer timeout')
    cells.environment(value['environment'])
    checks = value['checks']
    require(isinstance(checks, list) and 0 < len(checks) <= 10000, 'procedure checks required')
    seen = set()
    for check in checks:
        require(isinstance(check, dict) and check.keys() == {'id','selector'} and ID.fullmatch(check['id']) and check['id'] not in seen
                and isinstance(check['selector'], str) and 0 < len(check['selector']) <= 512, 'invalid/duplicate procedure check')
        seen.add(check['id'])
    artifacts = value['artifacts']
    require(isinstance(artifacts, list) and 0 < len(artifacts) <= 4096, 'procedure artifact set required')
    for item in artifacts:
        require(isinstance(item, dict) and item.keys() == {'role','kind','path'} and item['kind'] in ARTIFACT_KINDS, 'invalid procedure artifact')
    # Commands are argv vectors, never shell snippets. Manual and performance
    # producers use explicit retained inputs and dedicated checked adapters.
    program = basename(command[0]).lower()
    if value['producer_kind'] in {'rust_tests','host_contract'}:
        require(program in {'cargo','cargo.exe'} and command[1:2] == ['test'], 'Rust producer must execute cargo test')
    elif value['producer_kind'] == 'python_tests':
        require(program in {'python','python.exe','python3','python3.exe'} and command[1:3] == ['-m','unittest'] and ('-v' in command or '--verbose' in command), 'Python producer requires verbose unittest execution')
    elif value['producer_kind'] == 'packaging':
        require(program in {'pwsh','pwsh.exe','powershell','powershell.exe'} and '-File' in command, 'package producer requires a script argv')
        script = command[command.index('-File')+1]
        require(reference(script) == reference(repo_file('packaging/windows/verify-release.ps1')), 'package producer must run the complete final verifier')
    else:
        require(program in {'python','python.exe','python3','python3.exe'} and len(command) == 5
                and reference(command[1]) == reference(repo_file('tests/e2e/typed_producers.py')) and command[2:4] == ['inspect','--plan'],
                'manual/performance producers require the retained-input inspector')
    return value

def manual_observations(plan, artifacts):
    attestations = [item for item in artifacts if item['role'] == 'manual_attestation' and item['kind'] == 'report']
    require(len(attestations) == 1, 'manual attestation artifact required')
    data = read_json(local(attestations[0]['path']))
    require(isinstance(data, dict) and data.keys() == {'schema_version','kind','observer','observed_at_utc','environment','source_identity','observations'}
            and type(data['schema_version']) is int and data['schema_version'] == 1 and data['kind'] == 'manual_observation', 'invalid manual observation')
    require(isinstance(data['observer'], str) and data['observer'].strip() and len(data['observer']) <= 200, 'manual observer required')
    from datetime import datetime
    instant = datetime.fromisoformat(data['observed_at_utc'].replace('Z','+00:00'))
    require(instant.utcoffset() is not None, 'manual observation needs timezone')
    require(data['environment'] == plan['environment'], 'manual environment differs from procedure')
    cells.source_identity(data['source_identity'])
    observations = data['observations']
    require(isinstance(observations, list) and 0 < len(observations) <= 10000, 'manual observations missing')
    indexed = {}
    artifact_roles = {item['role'] for item in artifacts if item['role'] != 'manual_attestation'}
    for row in observations:
        require(isinstance(row, dict) and row.keys() == {'id','status','observed','artifact_roles'} and row['id'] not in indexed
                and row['status'] in {'PASS','FAIL','NOT_RUN'} and isinstance(row['observed'], str) and row['observed'].strip(), 'invalid manual observation row')
        require(isinstance(row['artifact_roles'], list) and set(row['artifact_roles']) <= artifact_roles
                and (row['status'] != 'PASS' or row['artifact_roles']), 'passing manual observation needs retained artifacts')
        indexed[row['id']] = row
    result = []
    for check in plan['checks']:
        row = indexed.get(check['selector'])
        result.append({'id':check['id'], 'status':row['status'] if row else 'NOT_RUN',
            'observed':f"Manual declaration by {data['observer']} at {data['observed_at_utc']}: " + (row['observed'] if row else 'observation absent')})
    return result

def performance_observations(plan, artifacts):
    by_role = {item['role']:item for item in artifacts}
    require('performance_report' in by_role and 'performance_provenance' in by_role, 'performance report and provenance required')
    report = read_json(local(by_role['performance_report']['path']))
    require(type(report.get('schema_version')) is int and report['schema_version'] == 2
            and report.get('kind') == 'performance_qualification_report', 'unsupported performance report')
    sys.path.insert(0, str(ROOT/'tests/perf'))
    import perf_suite
    require(perf_suite.report_source_valid(report), 'performance source changed or unavailable')
    provenance = report['provenance']
    cells.source_identity(provenance['source_before'])
    qualification = provenance['manifest']['qualification_environment']
    perf_suite.validate_qualification_environment(qualification)
    require(plan['environment']['hardware'] == canonical(qualification['hardware']).decode(), 'performance hardware differs from required environment')
    observed_os = qualification['os']
    family = {'Windows':'windows','Linux':'linux','Darwin':'macos'}.get(observed_os['name'])
    require(family == plan['environment']['os_family'] and observed_os['version'] == plan['environment']['os_build'], 'performance OS differs from required environment')
    hashes = {item['sha256'] for item in artifacts if item['kind'] in {'windows_executable','host_executable'}}
    for app in provenance['manifest']['applications'].values():
        require(app['sha256'] in hashes, 'measured executable missing from artifact set')
    by_name = {}
    for item in artifacts:
        if item['kind'] == 'report' and item['role'] != 'performance_report':
            name = basename(item['path'])
            require(name not in by_name, 'duplicate performance raw filename')
            by_name[name] = item
    require('provenance.json' in by_name, 'raw performance provenance required')
    for row in report['coverage']:
        if row.get('artifact') is not None:
            require(row['artifact'] in by_name and by_name[row['artifact']]['sha256'] == row['artifact_sha256'], 'raw performance trial missing/changed')
            raw = read_json(local(by_name[row['artifact']]['path']))
            if raw['status'] == 'ok':
                require(raw['exit_code'] == 0 and perf_suite.measurement(raw['stdout']) == raw['metrics'], 'persisted metrics differ from captured measurement event')
    # Re-run the official aggregation using only retained raw files. This checks
    # counts, missing trials, percentiles and eligibility without running apps.
    with tempfile.TemporaryDirectory(prefix='bareline-performance-recheck-') as directory:
        directory = Path(directory)
        for name, item in by_name.items():
            read_json(local(item['path']))  # Reject duplicate JSON fields before official aggregation.
            (directory/name).write_bytes(read_bounded_bytes(local(item['path']), 16*1024*1024))
        destination = directory/'recomputed-report.json'
        perf_suite.report(directory, destination)
        require(read_json(destination) == report, 'performance aggregate differs from retained raw trials')
    require(report.get('claims_eligible') is False, 'performance claim approval is a separate review')
    rows = {row['id']:row for row in report['observations']}
    result = []
    for check in plan['checks']:
        row = rows.get(check['selector'])
        status = 'PASS' if row and row['status'] == 'measured' else 'NOT_RUN' if row is None or row['status'] == 'not_run' else 'FAIL'
        result.append({'id':check['id'], 'status':status, 'observed':
            f"Measured evidence {check['selector']}: " + (json.dumps(row, sort_keys=True) if row else 'absent') + '; timing is informational; no comparative claim approved'})
    return result

def test_observations(kind, raw, exit_code, checks):
    text = raw.decode('utf-8', 'replace').replace('\r\n', '\n')
    if kind in {'rust_tests','host_contract'}:
        parsed = re.findall(r'^test ([^\n]+?) \.\.\. (ok|FAILED|ignored)(?:,.*)?$', text, re.M)
        summaries = re.findall(r'^test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;', text, re.M)
        totals = sum(sum(map(int, row)) for row in summaries)
        require(summaries and totals == len(parsed), 'missing, truncated or inconsistent per-test Rust output')
    else:
        parsed = [(full, status) for _, full, status in re.findall(r'^(\S+) \(([^\n]+)\)(?: \.\.\. |\n[^\n]{1,4096}? \.\.\. )(ok|FAIL|ERROR|skipped[^\n]*|unexpected success|expected failure)$', text, re.M)]
        count = re.findall(r'^Ran (\d+) tests? in ', text, re.M)
        require(len(count) == 1 and int(count[0]) == len(parsed), 'missing/truncated verbose unittest output')
    names = [name for name, _ in parsed]
    require(len(names) == len(set(names)), 'ambiguous repeated test names')
    observed = dict(parsed)
    result = []
    for check in checks:
        outcome = observed.get(check['selector'])
        status = 'NOT_RUN' if outcome is None or outcome.startswith(('ignored','skipped')) else 'PASS' if outcome == 'ok' and exit_code == 0 else 'FAIL'
        result.append({'id':check['id'], 'status':status, 'observed':f"{check['selector']}: {outcome or 'absent'}; producer exit={exit_code}"})
    return result

def artifact_policy(plan, artifacts):
    kinds = {item['kind'] for item in artifacts}
    if plan['producer_kind'] in {'rust_tests','python_tests','host_contract'}:
        require('source' in kinds or 'host_executable' in kinds, 'test evidence must identify tested source or host artifact')
    if plan['producer_kind'] == 'packaging':
        command = plan['command']
        options = {command[index].casefold():command[index+1] for index in range(len(command)-1)
                   if command[index].startswith('-')}
        require('-artifactdir' in options and '-releaseconfig' in options and '-authorityverifier' in options, 'final package invocation inputs missing')
        directory_name = options['-artifactdir']
        paths = {local(item['path']) for item in artifacts}
        context = RETAINED.get()
        if context:
            members = [path for name,path in context['objects'].items() if namespace().dirname(name) == reference(directory_name)]
            require(members and all(path in paths for path in members), 'retained package inputs missing from pinned set')
        else:
            directory = local(directory_name)
            require(directory.is_dir() and all(path.is_file() and not path.is_symlink() and not path.is_junction()
                    and path.resolve() in paths for path in directory.iterdir()), 'every final package input must be pinned')
        require(local(options['-releaseconfig']) in paths and local(options['-authorityverifier']) in paths, 'package config/verifier bytes must be pinned')
        require({'package','configuration','host_executable'} <= kinds, 'package evidence artifact policy incomplete')

def evaluate(plan, raw, exit_code, artifacts):
    kind = plan['producer_kind']
    if kind in {'rust_tests','python_tests','host_contract'}:
        return test_observations(kind, raw, exit_code, plan['checks'])
    if kind == 'packaging':
        require([check['selector'] for check in plan['checks']] == ['signed_final_assets'], 'package check must cover final verification')
        expected = 'Inventory, publisher pins, packaged authority, signed update/runtime/catalog metadata and actual payload bytes verified.'
        passed = exit_code == 0 and raw.decode('utf-8', 'replace').splitlines().count(expected) == 1
        return [{'id':plan['checks'][0]['id'], 'status':'PASS' if passed else 'FAIL', 'observed':expected if passed else f'Final asset verification failed or incomplete; exit={exit_code}'}]
    checks = manual_observations(plan, artifacts) if kind == 'manual' else performance_observations(plan, artifacts)
    require(loads(raw.strip()) == {'kind':'typed_input_inspection','producer_kind':kind,'checks':checks}, 'input inspection differs from captured producer output')
    require(exit_code == 0 or any(check['status'] != 'PASS' for check in checks), 'failed inspection cannot produce PASS')
    return checks

def checked_result(path, receipt_checker=None):
    if receipt_checker is None:
        receipt_checker = checked_receipt
    result = read_json(path)
    expected = {'schema_version','kind','procedure','procedure_sha256','procedure_file','procedure_file_sha256','producer_kind','environment','source_identity',
                'artifacts','artifact_set_sha256','test_receipt','test_receipt_sha256','checks','status'}
    require(isinstance(result, dict) and result.keys() == expected and type(result['schema_version']) is int
            and result['schema_version'] == 1 and result['kind'] == 'typed_producer_result', 'invalid typed result')
    plan = procedure(result['procedure'])
    require(digest(local(result['procedure_file'])) == result['procedure_file_sha256']
            and read_json(local(result['procedure_file'])) == plan, 'captured procedure file changed')
    require(result['procedure_sha256'] == hashlib.sha256(canonical(plan)).hexdigest() and result['producer_kind'] == plan['producer_kind'], 'typed procedure changed')
    require(result['environment'] == plan['environment'], 'typed environment changed')
    cells.environment(result['environment']); cells.source_identity(result['source_identity'])
    require(artifact_set(result['artifacts']) == result['artifact_set_sha256'], 'artifact set changed')
    artifact_policy(plan, result['artifacts'])
    require([{key:item[key] for key in ('role','kind','path')} for item in result['artifacts']] == plan['artifacts'], 'procedure artifact binding changed')
    for item in result['artifacts']:
        artifact = local(item['path'])
        require(artifact.is_file() and artifact.stat().st_size == item['bytes'] and digest(artifact) == item['sha256'], 'typed artifact bytes changed')
    receipt_path = local(result['test_receipt'])
    require(digest(receipt_path) == result['test_receipt_sha256'], 'typed receipt changed')
    receipt, raw = receipt_checker(receipt_path, allow_failed=True)
    require(receipt['source_before'] == result['source_identity'] and receipt['command'] == plan['command'], 'typed command/source differs from capture')
    if result['producer_kind'] in {'manual','performance'}:
        role = 'manual_attestation' if result['producer_kind'] == 'manual' else 'performance_report'
        item = next(item for item in result['artifacts'] if item['role'] == role)
        captured = read_json(local(item['path']))
        source = captured['source_identity'] if role == 'manual_attestation' else captured['provenance']['source_before']
        require(source == result['source_identity'], 'observed candidate source differs from current producer')
    computed = evaluate(plan, raw, receipt['exit_code'], result['artifacts'])
    require(computed == result['checks'], 'typed checks differ from actual producer observations')
    status = 'PASS' if all(check['status'] == 'PASS' for check in computed) else 'FAIL' if any(check['status'] == 'FAIL' for check in computed) else 'NOT_RUN'
    require(result['status'] == status, 'typed status contradicts producer')
    return result

def checked_receipt(path, allow_failed=False):
    if RETAINED.get() is None:
        import runner
        return runner.checked_test_receipt(path, allow_failed=allow_failed)
    receipt = read_json(local(path))
    require(type(receipt.get('schema_version')) is int and receipt['schema_version'] == 1
            and receipt.get('status') == 'completed' and type(receipt.get('exit_code')) is int
            and (allow_failed or receipt['exit_code'] == 0), 'retained producer did not complete successfully')
    passed = int(receipt['exit_code'] == 0)
    require(all(type(receipt.get(key)) is int for key in ('top_level_total','top_level_passed','top_level_failed'))
            and (receipt['top_level_total'],receipt['top_level_passed'],receipt['top_level_failed']) == (1,passed,1-passed)
            and receipt.get('summary_lines_are_not_aggregate_counts') is True, 'retained receipt accounting differs')
    require(receipt.get('source_changed_during_run') is False and receipt.get('source_before') == receipt.get('source_after'), 'retained producer source changed')
    cells.source_identity(receipt['source_before'])
    raw = []
    for name in ('stdout','stderr'):
        stream = receipt[name]
        data = read_bounded_bytes(local(stream['path']),16*1024*1024)
        require(hashlib.sha256(data).hexdigest() == stream['sha256'], 'retained raw output changed')
        raw.append(data)
    return receipt,b'\n'.join(raw)

def actual_environment(requested, manual=False):
    value = cells.environment(requested)
    actual = {'Windows':'windows','Linux':'linux','Darwin':'macos'}.get(platform.system())
    require(actual == value['os_family'], 'requested OS differs from actual execution host')
    require(value['os_build'] == platform.version(), 'requested OS build differs from actual execution host')
    architecture = 'arm64' if platform.machine().lower() in {'aarch64','arm64'} else 'x64' if platform.machine().lower() in {'amd64','x86_64'} else None
    require(value['architecture'] == architecture, 'requested architecture differs from actual host')
    require(manual or (value['mode'] == 'headless' and value['renderer'] == 'not_applicable'
            and value['assistive_technology'] == 'none'), 'automated typed producer cannot claim physical UI/AT observations')
    return value

def checked_mapping(path, evidence_id, plan):
    import runner
    document = read_json(path)
    require(type(document.get('schema_version')) is int and document['schema_version'] == 1
            and document.get('kind') == 'typed_qualification_mappings' and isinstance(document.get('mappings'), list), 'invalid typed mapping authority')
    selected = [row for row in document['mappings'] if row.get('id') == evidence_id and row.get('procedure_id') == plan['id']]
    require(len(selected) == 1, 'missing/ambiguous complete typed mapping')
    mapping = selected[0]
    require(mapping.keys() == {'id','procedure_id','producer_kind','required_checks','scope','reviewed','implementer','reviewer'}
            and mapping['producer_kind'] == plan['producer_kind'] and mapping['reviewed'] is True
            and isinstance(mapping['scope'], str) and mapping['scope'].strip(), 'typed mapping has not been completely reviewed')
    runner.require_independent_reviewer(mapping['implementer'], mapping['reviewer'])
    checks = mapping['required_checks']
    require(isinstance(checks, list) and checks and len(checks) == len(set(checks))
            and set(checks) <= {row['id'] for row in plan['checks']}, 'mapped check set missing or duplicated')
    require(evidence_id not in {'AC-021-01','AC-021-03'}, 'final closure criteria require a prerequisite-bound closure record')
    if evidence_id.startswith('command:'):
        require(plan['producer_kind'] == 'manual', 'runtime command outcomes require native or complete manual observations')
    return mapping

def validate_record(record):
    import runner
    key = record['id']
    runner.require_independent_reviewer(record.get('implementer'), record.get('reviewer'))
    result_path = local(record['result'])
    require(digest(result_path) == record['result_sha256'], 'typed result hash changed')
    result = checked_result(result_path)
    outer_path = local(record['test_receipt'])
    require(digest(outer_path) == record['test_receipt_sha256'], 'typed outer receipt changed')
    outer, raw = checked_receipt(outer_path, allow_failed=result['status'] != 'PASS')
    require(outer['source_before'] == result['source_identity'], 'typed wrapper source mismatch')
    command = outer['command']
    require(len(command) == 7 and basename(command[0]).lower() in {'python','python.exe','python3','python3.exe'}
            and reference(command[1]) == reference(repo_file('tests/e2e/typed_producers.py')) and command[2:4] == ['execute','--plan']
            and command[5] == '--output' and reference(command[4]) == reference(result['procedure_file'])
            and reference(command[6]) == reference(record['result']), 'typed result requires its actual capture wrapper')
    bindings = []
    for line in raw.decode('utf-8','replace').splitlines():
        try:
            value = loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict) and 'typed_result' in value:
            bindings.append(value)
    require(len(bindings) == 1 and reference(bindings[0]['typed_result']) == reference(record['result'])
            and bindings[0]['sha256'] == record['result_sha256'], 'typed result is not bound to captured output')
    mapping_path = local(record['mapping_source'])
    require(digest(mapping_path) == record['mapping_source_sha256'], 'typed mapping authority changed')
    mapping = checked_mapping(mapping_path, key, result['procedure'])
    if result['producer_kind'] == 'manual':
        item = next(item for item in result['artifacts'] if item['role'] == 'manual_attestation')
        runner.require_independent_reviewer(read_json(local(item['path']))['observer'], record['reviewer'])
    selected = [row for row in result['checks'] if row['id'] in mapping['required_checks']]
    status = 'PASS' if result['status'] == 'PASS' and all(row['status'] == 'PASS' for row in selected) else 'FAIL' if result['status'] == 'FAIL' else 'NOT_RUN'
    require(record['mapping'] == mapping and record['producer_kind'] == result['producer_kind']
            and record['source_identity'] == result['source_identity'] and record['artifact_set_sha256'] == result['artifact_set_sha256']
            and record['status'] == status and record['observed'] == selected, 'typed evidence disagrees with revalidated producer')
    require(all(record.get(field) == value for field, value in result['environment'].items()), 'typed record environment changed')
    return result

def adapt(result_path, receipt_path, mapping_path, evidence_id, cell_id, output):
    result = checked_result(local(result_path))
    mapping = checked_mapping(local(mapping_path), evidence_id, result['procedure'])
    selected = [row for row in result['checks'] if row['id'] in mapping['required_checks']]
    record = {'id':evidence_id, 'producer_kind':result['producer_kind'], 'cell_id':cell_id,
        'implementer':mapping['implementer'],'reviewer':mapping['reviewer'],
        'result':str(local(result_path)), 'result_sha256':digest(local(result_path)),
        'test_receipt':str(local(receipt_path)), 'test_receipt_sha256':digest(local(receipt_path)),
        'mapping_source':str(local(mapping_path)), 'mapping_source_sha256':digest(local(mapping_path)), 'mapping':mapping,
        'source_identity':result['source_identity'], 'artifact_set_sha256':result['artifact_set_sha256'],
        'status':result['status'], 'observed':selected, **result['environment']}
    validate_record(record)
    with local(output).open('x', encoding='utf-8') as stream:
        json.dump({'schema_version':1,'evidence':[record]}, stream, indent=2)

def execute(plan_path, output):
    plan = procedure(read_json(plan_path)); actual_environment(plan['environment'], plan['producer_kind'] == 'manual')
    output = local(output)
    require(output.is_relative_to(ROOT/'target') and not output.exists(), 'use new ignored target output for typed capture')
    records = []
    for item in plan['artifacts']:
        path = local(item['path'])
        require(path.is_file() and path.stat().st_size <= 1024**3, 'missing/bounded producer input required')
        records.append(item | {'sha256':digest(path),'bytes':path.stat().st_size})
    artifact_set(records)
    artifact_policy(plan, records)
    spec = importlib.util.spec_from_file_location('typed_t09', ROOT/'.github/workflows/run_test_evidence.py')
    module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
    receipt_path = output.with_suffix('.producer-receipt.json')
    receipt = module.run_receipt(receipt_path, plan['command'], ROOT, plan['timeout_seconds'])
    import runner
    checked, raw = runner.checked_test_receipt(receipt_path, allow_failed=True)
    for item in records:
        require(digest(local(item['path'])) == item['sha256'], 'producer input changed during execution')
    observations = evaluate(plan, raw, checked['exit_code'], records)
    status = 'PASS' if all(row['status'] == 'PASS' for row in observations) else 'FAIL' if any(row['status'] == 'FAIL' for row in observations) else 'NOT_RUN'
    result = {'schema_version':1,'kind':'typed_producer_result','procedure':plan,
        'procedure_file':str(local(plan_path)), 'procedure_file_sha256':digest(local(plan_path)),
        'procedure_sha256':hashlib.sha256(canonical(plan)).hexdigest(), 'producer_kind':plan['producer_kind'],
        'environment':plan['environment'],'source_identity':receipt['source_before'],'artifacts':records,
        'artifact_set_sha256':artifact_set(records),'test_receipt':str(receipt_path),'test_receipt_sha256':digest(receipt_path),
        'checks':observations,'status':status}
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open('x', encoding='utf-8') as stream:
        json.dump(result, stream, indent=2)
    checked_result(output)
    print(json.dumps({'typed_result':str(output), 'sha256':digest(output)}))
    return 0 if status == 'PASS' else 1

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    run = sub.add_parser('execute'); run.add_argument('--plan', type=Path, required=True); run.add_argument('--output', type=Path, required=True)
    check = sub.add_parser('verify'); check.add_argument('--result', type=Path, required=True)
    inspect = sub.add_parser('inspect'); inspect.add_argument('--plan', type=Path, required=True)
    adapter = sub.add_parser('adapt')
    for name in ('result','receipt','mapping','output'):
        adapter.add_argument('--'+name, type=Path, required=True)
    adapter.add_argument('--evidence-id', required=True); adapter.add_argument('--cell-id', required=True)
    args = parser.parse_args()
    try:
        if args.command == 'execute': return execute(args.plan, args.output)
        if args.command == 'adapt':
            adapt(args.result, args.receipt, args.mapping, args.evidence_id, args.cell_id, args.output)
            return 0
        if args.command == 'inspect':
            plan = procedure(read_json(args.plan))
            require(plan['producer_kind'] in {'manual','performance'}, 'inspection producer kind required')
            artifacts = [item | {'sha256':digest(local(item['path'])),'bytes':local(item['path']).stat().st_size} for item in plan['artifacts']]
            checks = manual_observations(plan, artifacts) if plan['producer_kind'] == 'manual' else performance_observations(plan, artifacts)
            print(json.dumps({'kind':'typed_input_inspection','producer_kind':plan['producer_kind'],'checks':checks}))
            return 0 if all(check['status'] == 'PASS' for check in checks) else 1
        checked_result(args.result)
        print('PASS: producer observations revalidated; acceptance requires complete reviewed mapping')
        return 0
    except (ValueError, OSError, KeyError) as error:
        print(f'typed evidence error: {error}', file=sys.stderr)
        return 2

if __name__ == '__main__':
    raise SystemExit(main())
