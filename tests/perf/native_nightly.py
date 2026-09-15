# SPDX-License-Identifier: MPL-2.0
"""Full native registry for an explicitly provisioned, dedicated Windows runner.

Missing prerequisites produce coverage receipts, never fabricated measurements.
No work occurs at import; this file does not enable or schedule its own execution.
"""
import argparse
import os
from pathlib import Path
import platform
import sys

import perf_suite as suite


def required_cases():
    dual = {'cold_launch', 'warm_launch', 'empty_idle', 'scroll'}
    return [(name, renderer) for name in suite.SCENARIOS
            for renderer in (('hardware', 'software') if name in dual else ('hardware',))]


def regression_rows(report, scenario, renderer, repetitions):
    if report.get('failures') or report.get('missing_trials') or not suite.report_source_valid(report):
        return []
    return [{'id': scenario + '@' + renderer + '/' + value['metric'],
             'scenario': scenario + '@' + renderer, 'metric': value['metric'],
             'sample_count': value['sample_count'], 'sample_scope': 'native-only dedicated runner',
             'bareline': {'p50': value['p50'], 'p95': value['p95']},
             'notepadpp': {'p50': None, 'p95': None}, 'p50_ratio': None,
             'status': 'measured'}
            for value in report.get('observations', [])
            if value['application'] == 'bareline' and value['sample_count'] == repetitions
            and value.get('status') == 'measured']


def pinned(record):
    if not isinstance(record, dict) or type(record.get('path')) is not str:
        raise ValueError('pinned prerequisite needs a string path and SHA-256')
    path = Path(record['path']).resolve(strict=True)
    if suite.digest(path) != suite.require_sha256(record.get('sha256'), 'prerequisite sha256'):
        raise ValueError('prerequisite hash mismatch: ' + str(path))
    return path


def qualification_exit_code(source_before, source_after, coverage):
    valid_source = (isinstance(source_before, dict) and source_before.get('available') is True
                    and isinstance(source_after, dict) and source_after.get('available') is True
                    and source_before == source_after)
    return 0 if valid_source and all(case['status'] == 'measured' for case in coverage) else 1


def run(plan_path, plan_sha256, application, destination):
    destination = Path(destination).resolve()
    destination.mkdir(parents=True, exist_ok=False)
    source_root = Path(__file__).resolve().parents[2]
    source_before = suite.t09_source_identity(source_root, (destination,))
    plan = {}
    qualification_environment = None
    application_path = str(Path(application).resolve())
    application_sha = None
    version = None
    prerequisite_error = None
    try:
        if not plan_path or suite.digest(plan_path) != suite.require_sha256(plan_sha256, 'plan sha256'):
            raise ValueError('dedicated native plan is missing or its pin does not match')
        plan = suite.read_json(plan_path)
        if plan.get('schema_version') != 1 or plan.get('interactive_desktop_ready') is not True:
            raise ValueError('plan must explicitly confirm a dedicated interactive desktop')
        if type(plan.get('machine_id')) is not str or plan['machine_id'].casefold() != platform.node().casefold():
            raise ValueError('plan machine identity does not match this host')
        if type(plan['repetitions']) is not int or plan['repetitions'] != 3:
            raise ValueError('the bounded full nightly registry requires exactly three repetitions')
        qualification_environment = suite.validate_qualification_environment(
            plan['qualification_environment'], Path(plan_path).resolve().parent)
        for font in qualification_environment['fonts']:
            font['path'] = str((Path(plan_path).resolve().parent / font['path']).resolve())
        settings = pinned(plan['settings'])
        application = Path(application).resolve(strict=True)
        application_path = str(application)
        from notepadpp_driver import file_version, require_x64_pe
        require_x64_pe(application)
        version = file_version(application)
        application_sha = suite.digest(application)
    except (OSError, ValueError, KeyError, TypeError) as error:
        prerequisite_error = str(error)
    coverage = []
    rows = []
    directory = Path(__file__).resolve().parent
    for name, renderer in required_cases():
        record = {'id': name + '@' + renderer, 'scenario': name, 'renderer': renderer, 'status': 'unavailable'}
        try:
            if prerequisite_error:
                raise ValueError(prerequisite_error)
            args = [str(Path(sys.executable).resolve()), str(directory / 'bareline_driver.py'),
                    '--application', '{application}', '--sha256', application_sha,
                    '--version', version, '--config', str(settings),
                    '--config-sha256', plan['settings']['sha256'], '--scenario', name, '--renderer', renderer]
            fixtures = []
            if name not in ('cold_launch', 'warm_launch', 'empty_idle', 'tabs_100', 'tabs_500'):
                fixture_record = plan['fixtures'][name]
                fixture = pinned(fixture_record)
                suite.validate_fixture_size(fixture, name)
                args += ['--fixture', str(fixture), '--fixture-sha256', fixture_record['sha256']]
                fixtures = [{'path': str(fixture), 'sha256': fixture_record['sha256']}]
            if name == 'cold_launch':
                cold = plan['cold_cache']
                args += ['--cache-plan', str(pinned(cold)), '--cache-plan-sha256', cold['sha256']]
            if name == 'extensions_memory':
                extension = plan['extensions']
                args += ['--extension-inventory', str(pinned(extension)), '--extension-inventory-sha256', extension['sha256'],
                         '--extension-command', extension['command']]
            support = [{'path': str(directory / source), 'sha256': suite.digest(directory / source)} for source in
                       ('bareline_driver.py', 'notepadpp_driver.py', 'windows_process_metrics.py',
                        'disk_metrics.py', 'cache_protocol.py', 'perf_suite.py')]
            support.append({'path': str(suite.SOURCE_IDENTITY_TOOL),
                            'sha256': suite.digest(suite.SOURCE_IDENTITY_TOOL)})
            scenario = {'name': name, 'renderer': renderer, 'cache_state': 'external cold receipt' if name == 'cold_launch'
                        else 'one warm-up launch' if name == 'warm_launch' else 'uncontrolled/warmed fixture',
                        'cache_plan_sha256': plan['cold_cache']['sha256'] if name == 'cold_launch' else None,
                        'timeout_seconds': 300, 'fixtures': fixtures, 'comparable_metrics': [],
                        'drivers': {'bareline': {'argv': args, 'sha256': suite.digest(sys.executable), 'pinned_files': support}}}
            manifest = {'schema_version': 2, 'series': 'local', 'configuration': plan['configuration'],
                        'machine_id': plan['machine_id'], 'repetitions': plan['repetitions'],
                        'qualification_environment': qualification_environment,
                        'source_identity': {'algorithm': 't09-run-test-evidence-v1', 'root': str(source_root),
                                            'excluded_paths': [str(destination)], 'before': source_before},
                        'applications': {'bareline': {'executable': str(application), 'sha256': application_sha,
                            'version': version, 'settings': plan['settings']}}, 'scenarios': [scenario]}
            manifest_path = destination / (name + '-' + renderer + '-manifest.json')
            suite.write_new(manifest_path, manifest)
            record['status'] = 'running'
            run_directory = suite.run(manifest_path, destination / 'raw')
            report_path = destination / (name + '-' + renderer + '-report.json')
            suite.report(run_directory, report_path)
            report = suite.read_json(report_path)
            rows += regression_rows(report, name, renderer, plan['repetitions'])
            failed = bool(report['failures'] or report['missing_trials'])
            source_invalid = not suite.report_source_valid(report)
            record.update(status='failed' if failed else 'unqualified' if source_invalid else 'measured',
                          report=str(report_path.relative_to(destination)), report_sha256=suite.digest(report_path))
            if failed:
                record['reason'] = 'report contains failed or missing trials'
            elif source_invalid:
                record['reason'] = 'source identity is unavailable or changed during the case'
        except (OSError, ValueError, RuntimeError, KeyError, TypeError) as error:
            if record['status'] == 'running':
                record['status'] = 'failed'
            record['reason'] = str(error)
        coverage.append(record)
    source_after = suite.t09_source_identity(source_root, (destination,))
    source_changed = source_after != source_before
    ineligibility = ['comparator application is absent', 'owner review and approval are pending']
    if any(case['status'] != 'measured' for case in coverage):
        ineligibility.append('one or more registry cases failed or are unavailable')
    if source_changed or source_before.get('available') is not True:
        ineligibility.append('source identity is unavailable or changed during the run')
    result = {'schema_version': 2, 'kind': 'performance_qualification_report',
              'qualification_status': 'unqualified',
              'claims_eligible': False, 'ineligibility_reasons': ineligibility,
              'coverage': coverage, 'rows': rows,
              'failures': [case for case in coverage if case['status'] != 'measured'],
              'provenance': {'manifest': {'series': 'local', 'machine_id': plan.get('machine_id', platform.node()),
                  'commit': source_before.get('head') or os.environ.get('GITHUB_SHA'),
                  'source_manifest_sha256': source_before.get('source_manifest_sha256'),
                  'working_tree_dirty': source_before.get('working_tree_dirty'),
                  'source_before': source_before, 'source_after': source_after,
                  'source_changed_during_run': source_changed,
                  'application': {'path': application_path, 'sha256': application_sha, 'version': version},
                  'configuration': {'profile_id': plan.get('configuration'),
                                    'qualification_environment': qualification_environment,
                                    'settings': plan.get('settings'),
                                    'fixtures': plan.get('fixtures'), 'cold_cache': plan.get('cold_cache'),
                                    'extensions': plan.get('extensions'), 'os': platform.platform(),
                                    'arch': platform.machine(), 'python': platform.python_version(),
                                    'measurement_profile': 'owned Job memory20ms/disk100ms',
                                    'series_kind': 'dedicated-native-nightly'},
                  'scenarios': [{'name': name, 'renderer': renderer} for name, renderer in required_cases()]},
                  'plan_sha256': plan_sha256 or None},
              'limitations': 'Dedicated native observations only. Unavailable/failed cases retain reasons; no benchmark acceptance is inferred.'}
    suite.write_new(destination / 'native-report.json', result)
    return qualification_exit_code(source_before, source_after, coverage)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--plan', default='')
    parser.add_argument('--plan-sha256', default='')
    parser.add_argument('--application', required=True)
    parser.add_argument('--destination', required=True)
    args = parser.parse_args()
    return run(args.plan, args.plan_sha256, args.application, args.destination)


if __name__ == '__main__':
    raise SystemExit(main())
