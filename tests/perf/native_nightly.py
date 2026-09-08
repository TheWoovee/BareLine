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
    if report.get('failures') or report.get('missing_trials'):
        return []
    return [{'scenario': scenario + '@' + renderer, 'metric': value['metric'],
             'sample_count': value['sample_count'], 'sample_scope': 'native-only dedicated runner',
             'bareline': {'p50': value['p50'], 'p95': value['p95']},
             'notepadpp': {'p50': None, 'p95': None}, 'p50_ratio': None}
            for value in report.get('observations', [])
            if value['application'] == 'bareline' and value['sample_count'] == repetitions]


def pinned(record):
    path = Path(record['path']).resolve(strict=True)
    if suite.digest(path) != record['sha256'].lower():
        raise ValueError('prerequisite hash mismatch: ' + str(path))
    return path


def run(plan_path, plan_sha256, application, destination):
    destination = Path(destination).resolve()
    destination.mkdir(parents=True, exist_ok=False)
    plan = {}
    prerequisite_error = None
    try:
        if not plan_path or suite.digest(plan_path) != plan_sha256.lower():
            raise ValueError('dedicated native plan is missing or its pin does not match')
        plan = suite.read_json(plan_path)
        if plan.get('schema_version') != 1 or plan.get('interactive_desktop_ready') is not True:
            raise ValueError('plan must explicitly confirm a dedicated interactive desktop')
        if plan['machine_id'].casefold() != platform.node().casefold():
            raise ValueError('plan machine identity does not match this host')
        if type(plan['repetitions']) is not int or plan['repetitions'] != 3:
            raise ValueError('the bounded full nightly registry requires exactly three repetitions')
        settings = pinned(plan['settings'])
        application = Path(application).resolve(strict=True)
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
        record = {'scenario': name, 'renderer': renderer, 'status': 'unavailable'}
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
            scenario = {'name': name, 'renderer': renderer, 'cache_state': 'external cold receipt' if name == 'cold_launch'
                        else 'one warm-up launch' if name == 'warm_launch' else 'uncontrolled/warmed fixture',
                        'timeout_seconds': 300, 'fixtures': fixtures, 'comparable_metrics': [],
                        'drivers': {'bareline': {'argv': args, 'sha256': suite.digest(sys.executable), 'pinned_files': support}}}
            manifest = {'schema_version': 1, 'series': 'local', 'configuration': plan['configuration'],
                        'machine_id': plan['machine_id'], 'repetitions': plan['repetitions'],
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
            record.update(status='failed' if report['failures'] or report['missing_trials'] else 'measured',
                          report=str(report_path.relative_to(destination)), report_sha256=suite.digest(report_path))
        except (OSError, ValueError, RuntimeError, KeyError, TypeError) as error:
            if record['status'] == 'running':
                record['status'] = 'failed'
            record['reason'] = str(error)
        coverage.append(record)
    result = {'schema_version': 1, 'claims_eligible': False, 'coverage': coverage, 'rows': rows,
              'failures': [case for case in coverage if case['status'] != 'measured'],
              'provenance': {'manifest': {'series': 'local', 'machine_id': plan.get('machine_id', platform.node()),
                  'commit': os.environ.get('GITHUB_SHA'),
                  'configuration': {'plan_configuration': plan.get('configuration'), 'settings': plan.get('settings'),
                                    'fixtures': plan.get('fixtures'), 'cold_cache': plan.get('cold_cache'),
                                    'extensions': plan.get('extensions'), 'os': platform.platform(),
                                    'arch': platform.machine(), 'python': platform.python_version(),
                                    'measurement_profile': 'owned Job memory20ms/disk100ms',
                                    'series_kind': 'dedicated-native-nightly'},
                  'scenarios': [{'name': name, 'renderer': renderer} for name, renderer in required_cases()]},
                  'plan_sha256': plan_sha256 or None},
              'limitations': 'Dedicated native observations only. Unavailable/failed cases retain reasons; no benchmark acceptance is inferred.'}
    suite.write_new(destination / 'native-report.json', result)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--plan', default='')
    parser.add_argument('--plan-sha256', default='')
    parser.add_argument('--application', required=True)
    parser.add_argument('--destination', required=True)
    args = parser.parse_args()
    run(args.plan, args.plan_sha256, args.application, args.destination)


if __name__ == '__main__':
    main()
