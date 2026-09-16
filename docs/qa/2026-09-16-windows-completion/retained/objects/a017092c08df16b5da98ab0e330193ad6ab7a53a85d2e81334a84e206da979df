# SPDX-License-Identifier: MPL-2.0
"""Two-stage acceptance closure; a readiness report never proves itself."""
import json
from pathlib import Path
from evidence_json import read_json, loads
import typed_producers as typed

CLOSURE_IDS = {'AC-021-01', 'AC-021-03'}

def checked_reference(reference):
    typed.require(isinstance(reference, dict) and reference.keys() == {'path','sha256'}, 'invalid closure artifact reference')
    path = typed.local(reference['path'])
    typed.require(typed.digest(path) == reference['sha256'], 'closure artifact changed')
    return path

def validate(path, expected):
    import runner
    closure = read_json(path)
    typed.require(isinstance(closure, dict) and closure.keys() == {'schema_version','kind','review','prerequisite_report','prerequisite_receipt','readiness_record'}
        and type(closure['schema_version']) is int and closure['schema_version'] == 1
        and closure['kind'] == 'release_acceptance_closure', 'invalid final closure record')
    review = closure['review']
    typed.require(isinstance(review, dict) and review.keys() == {'implementer','reviewer','reviewed_at_utc'}, 'closure review required')
    runner.require_independent_reviewer(review['implementer'], review['reviewer'])
    from datetime import datetime
    typed.require(datetime.fromisoformat(review['reviewed_at_utc'].replace('Z','+00:00')).utcoffset() is not None, 'review timestamp needs timezone')
    prerequisite_path = checked_reference(closure['prerequisite_report'])
    prerequisite = read_json(prerequisite_path)
    typed.require(prerequisite.get('stage') == 'prerequisites' and prerequisite.get('evidence_complete') is True
        and prerequisite.get('unresolved') == [] and set(prerequisite.get('closure_ids_deferred', [])) == CLOSURE_IDS
        and not (CLOSURE_IDS & set(prerequisite.get('resolved', {}))), 'closure requires a complete nonrecursive prerequisite report')
    typed.require(prerequisite.get('resolved') and all(value in {'PASS','EXCLUDED'} for value in prerequisite['resolved'].values()), 'prerequisite outcomes incomplete')
    for key, value in expected.items():
        typed.require(value is not None and prerequisite.get(key) == value, 'prerequisite identity differs from final candidate: '+key)
    receipt, raw = runner.checked_test_receipt(checked_reference(closure['prerequisite_receipt']))
    command = receipt['command']
    typed.require('--stage' in command and command[command.index('--stage')+1] == 'prerequisites'
        and '--output' in command and typed.local(command[command.index('--output')+1]) == prerequisite_path, 'prerequisite receipt did not run the prerequisite resolver')
    python_runner = len(command) >= 3 and Path(command[0]).name.lower() in {'python','python.exe','python3','python3.exe'} and typed.local(command[1]) == typed.ROOT/'tests/e2e/runner.py' and command[2] == 'resolve'
    cargo_runner = len(command) >= 4 and Path(command[0]).name.lower() in {'cargo','cargo.exe'} and command[1:4] == ['xtask','qa','resolve']
    typed.require(python_runner or cargo_runner, 'unknown prerequisite producer')
    bindings = []
    for line in raw.decode('utf-8','replace').splitlines():
        try: value = loads(line)
        except json.JSONDecodeError: continue
        if isinstance(value, dict) and 'coverage_report' in value: bindings.append(value)
    typed.require(len(bindings) == 1 and typed.local(bindings[0]['coverage_report']) == prerequisite_path
        and bindings[0]['sha256'] == closure['prerequisite_report']['sha256'], 'prerequisite report not bound to captured resolver output')
    typed.require(receipt['source_before'] == prerequisite['source_identity'], 'prerequisite resolver source differs from candidate')
    readiness_path = checked_reference(closure['readiness_record'])
    readiness = read_json(readiness_path)
    typed.require(type(readiness.get('schema_version')) is int and readiness['schema_version'] == 1
        and readiness.get('kind') == 'release_readiness_record'
        and readiness.get('prerequisite_report_sha256') == closure['prerequisite_report']['sha256']
        and readiness.get('parity_index_sha256') == prerequisite['index_sha256'], 'readiness record does not bind prerequisite/parity evidence')
    issues = readiness.get('correctness_issues')
    typed.require(isinstance(issues, list) and len(issues) <= 10000, 'explicit correctness register required')
    seen = set()
    for issue in issues:
        typed.require(isinstance(issue, dict) and all(isinstance(issue.get(key), str) and issue[key].strip()
            for key in ('id','severity','status','description','disposition')) and issue['id'] not in seen, 'invalid/duplicate correctness issue')
        seen.add(issue['id'])
        typed.require(issue['severity'] in {'P0','P1','P2','P3'} and issue['status'] in {'open','fixed','accepted_limit'}, 'invalid correctness disposition')
        typed.require(issue['severity'] not in {'P0','P1'} or issue['status'] == 'fixed', 'release closure contains an unresolved P0/P1 issue')
    reports = readiness.get('performance_evidence')
    typed.require(isinstance(reports, list) and reports, 'performance evidence and its limitations must be retained')
    for reference in reports:
        result = typed.checked_result(checked_reference(reference))
        typed.require(result['producer_kind'] == 'performance', 'performance evidence must use the checked producer')
    typed.require(isinstance(readiness.get('limitations'), list) and all(isinstance(item, str) and item.strip() for item in readiness['limitations']), 'explicit limitations required')
    claims = readiness.get('parity_claims')
    typed.require(isinstance(claims, list) and claims and all(isinstance(item, dict)
        and isinstance(item.get('claim'), str) and item['claim'].strip() and isinstance(item.get('evidence_ids'), list)
        and item['evidence_ids'] and all(prerequisite['resolved'].get(key) in {'PASS','EXCLUDED'} for key in item['evidence_ids'])
        for item in claims), 'parity claims must be limited to prerequisite evidence')
    typed.require(readiness.get('release_approved') is False, 'acceptance evidence closure does not authorize publication')
    return {key:'PASS' for key in CLOSURE_IDS}
