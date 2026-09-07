# SPDX-License-Identifier: MPL-2.0
"""Prepare pinned paired driver commands without launching an application."""
import argparse
from pathlib import Path
import sys
from perf_suite import digest, write_new

COMMON = ('open_10mb', 'open_100mb', 'open_1gb', 'open_5gb', 'long_line',
          'scroll', 'edit_to_paint', 'literal_search', 'regex_search', 'result_jump')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for application in ('bareline', 'notepadpp'):
        parser.add_argument('--' + application, required=True)
        parser.add_argument('--' + application + '-version', required=True)
        parser.add_argument('--' + application + '-config', required=True)
    parser.add_argument('--fixture', required=True)
    parser.add_argument('--expected-text-bytes', required=True, type=int)
    parser.add_argument('--scenario', choices=COMMON, action='append', required=True)
    parser.add_argument('--machine-id', required=True)
    parser.add_argument('--configuration', required=True, help='reviewed OS/CPU/display/DPI/theme/font/wrap/syntax/power settings identity')
    parser.add_argument('--renderer', choices=('hardware', 'software'), default='hardware')
    parser.add_argument('--repetitions', type=int, default=5)
    parser.add_argument('--destination', required=True)
    args = parser.parse_args()
    if not 1 <= args.repetitions <= 100 or args.expected_text_bytes < 0:
        raise ValueError('invalid repetition count or text-view length')
    directory = Path(__file__).resolve().parent
    fixture = Path(args.fixture).resolve()
    fixture_hash = digest(fixture)
    python = str(Path(sys.executable).resolve())
    support = [{"path": str(directory / name), "sha256": digest(directory / name)} for name in
               ('perf_suite.py', 'bareline_driver.py', 'notepadpp_driver.py', 'windows_process_metrics.py')]
    applications = {}
    for name in ('bareline', 'notepadpp'):
        executable = Path(getattr(args, name)).resolve()
        config = Path(getattr(args, name + '_config')).resolve()
        applications[name] = {"executable": str(executable), "sha256": digest(executable),
            "version": getattr(args, name + '_version'), "settings": {
                "config": str(config), "config_sha256": digest(config), "plugins_disabled": True}}
        support.append({"path": str(config), "sha256": digest(config)})
    scenarios = []
    for scenario in dict.fromkeys(args.scenario):
        drivers = {}
        for name, application in applications.items():
            config = application['settings']
            argv = [python, str(directory / (name + '_driver.py')), '--application', '{application}',
                    '--sha256', application['sha256'], '--version', application['version'],
                    '--config', config['config'], '--config-sha256', config['config_sha256'],
                    '--scenario', scenario, '--fixture', str(fixture), '--fixture-sha256', fixture_hash]
            if name == 'bareline':
                argv += ['--renderer', args.renderer]
            else:
                argv += ['--expected-text-bytes', str(args.expected_text_bytes), '--timeout', '120']
            drivers[name] = {"argv": argv, "sha256": digest(python), "pinned_files": support}
        scenarios.append({"name": scenario, "timeout_seconds": 150,
            "cache_state": "uncontrolled; hash validation and per-trial copy warm caches",
            "renderer": args.renderer, "fixtures": [{"path": str(fixture), "sha256": fixture_hash}],
            "comparable_metrics": [], "drivers": drivers})
    write_new(args.destination, {"schema_version": 1, "series": "local", "machine_id": args.machine_id,
        "configuration": args.configuration, "repetitions": args.repetitions,
        "applications": applications, "scenarios": scenarios,
        "comparison_review": "No default ratios: native presented/whole-find and Scintilla roundtrip endpoints differ. Review identical work and sampling boundaries before explicitly listing comparable_metrics."})


if __name__ == '__main__':
    main()
