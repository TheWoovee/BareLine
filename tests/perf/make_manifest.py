# SPDX-License-Identifier: MPL-2.0
"""Prepare pinned paired driver commands without launching an application."""
import argparse
from pathlib import Path
import sys
from perf_suite import (SOURCE_IDENTITY_TOOL, digest, read_json, t09_source_identity,
                        validate_fixture_size, validate_qualification_environment, write_new)

COMMON = ('open_10mb', 'open_100mb', 'open_1gb', 'open_5gb', 'long_line',
          'scroll', 'edit_to_paint', 'literal_search', 'regex_search', 'result_jump', 'save', 'warm_launch', 'cold_launch', 'empty_idle')
NATIVE = COMMON + ('save_as', 'syntax_viewport', 'search_cancel', 'workspace_scan', 'tail_append', 'extensions_memory', 'tabs_100', 'tabs_500')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--native-only', action='store_true')
    for application in ('bareline', 'notepadpp'):
        parser.add_argument('--' + application, required=application == 'bareline')
        parser.add_argument('--' + application + '-version', required=application == 'bareline')
        parser.add_argument('--' + application + '-config', required=application == 'bareline')
    parser.add_argument('--fixture', required=True)
    parser.add_argument('--expected-text-bytes', required=True, type=int)
    parser.add_argument('--scenario', choices=NATIVE, action='append', required=True)
    parser.add_argument('--cache-plan')
    parser.add_argument('--extension-inventory')
    parser.add_argument('--extension-command')
    parser.add_argument('--machine-id', required=True)
    parser.add_argument('--configuration', required=True, help='stable reviewed profile identifier')
    parser.add_argument('--qualification-environment', required=True,
                        help='JSON pinning hardware, OS, power, DPI, fonts, editor options, acquisition and cold method')
    parser.add_argument('--source-root', default=str(Path(__file__).resolve().parents[2]),
                        help='Git worktree recorded with the PR-T09 source identity algorithm')
    parser.add_argument('--renderer', choices=('hardware', 'software'), default='hardware')
    parser.add_argument('--repetitions', type=int, default=5)
    parser.add_argument('--destination', required=True)
    args = parser.parse_args()
    names = ('bareline',) if args.native_only else ('bareline', 'notepadpp')
    if not args.native_only and (any(s not in COMMON for s in args.scenario) or not all((args.notepadpp, args.notepadpp_version, args.notepadpp_config))):
        raise ValueError('paired preparation requires Notepad++ pins and mutually supported scenarios')
    if 'cold_launch' in args.scenario and not args.cache_plan:
        raise ValueError('cold launch requires an explicit cache preparation plan')
    if not 1 <= args.repetitions <= 100 or args.expected_text_bytes < 0:
        raise ValueError('invalid repetition count or text-view length')
    directory = Path(__file__).resolve().parent
    destination = Path(args.destination).resolve()
    environment_path = Path(args.qualification_environment).resolve()
    qualification_environment = validate_qualification_environment(read_json(environment_path), environment_path.parent)
    for font in qualification_environment['fonts']:
        font['path'] = str((environment_path.parent / font['path']).resolve())
    source_root = Path(args.source_root).resolve()
    source_before = t09_source_identity(source_root, (destination,))
    if source_before.get('available') is not True:
        raise ValueError('source identity unavailable: ' + source_before.get('reason', 'unknown reason'))
    fixture = Path(args.fixture).resolve()
    fixture_hash = digest(fixture)
    python = str(Path(sys.executable).resolve())
    support = [{"path": str(directory / name), "sha256": digest(directory / name)} for name in
               ('perf_suite.py', 'bareline_driver.py', 'notepadpp_driver.py', 'windows_process_metrics.py', 'cache_protocol.py', 'disk_metrics.py')]
    support += [{"path": str(SOURCE_IDENTITY_TOOL), "sha256": digest(SOURCE_IDENTITY_TOOL)},
                {"path": str(environment_path), "sha256": digest(environment_path)}]
    applications = {}
    for name in names:
        executable = Path(getattr(args, name)).resolve()
        config = Path(getattr(args, name + '_config')).resolve()
        applications[name] = {"executable": str(executable), "sha256": digest(executable),
            "version": getattr(args, name + '_version'), "settings": {
                "config": str(config), "config_sha256": digest(config), "plugins_disabled": True,
                "measurement_profile": "owned Job memory20ms/disk100ms"}}
        support.append({"path": str(config), "sha256": digest(config)})
    scenarios = []
    for scenario in dict.fromkeys(args.scenario):
        if scenario not in ('cold_launch', 'warm_launch', 'empty_idle', 'tabs_100', 'tabs_500'):
            validate_fixture_size(fixture, scenario)
        drivers = {}
        for name, application in applications.items():
            config = application['settings']
            argv = [python, str(directory / (name + '_driver.py')), '--application', '{application}',
                    '--sha256', application['sha256'], '--version', application['version'],
                    '--config', config['config'], '--config-sha256', config['config_sha256'],
                    '--scenario', scenario]
            if scenario not in ('cold_launch', 'warm_launch', 'empty_idle', 'tabs_100', 'tabs_500'):
                argv += ['--fixture', str(fixture), '--fixture-sha256', fixture_hash]
            if name == 'bareline':
                argv += ['--renderer', args.renderer]
            else:
                argv += ['--expected-text-bytes', str(args.expected_text_bytes), '--timeout', '120']
                if scenario in ('literal_search', 'regex_search'):
                    argv += ['--pattern', 'PERF_ABSENT_TOKEN']
            if scenario == 'cold_launch':
                cache_plan = str(Path(args.cache_plan).resolve())
                argv += ['--cache-plan', cache_plan, '--cache-plan-sha256', digest(cache_plan)]
            if scenario == 'extensions_memory':
                if not args.extension_inventory or not args.extension_command:
                    raise ValueError('extension scenario requires inventory and owner/command')
                inventory = str(Path(args.extension_inventory).resolve())
                argv += ['--extension-inventory', inventory, '--extension-inventory-sha256', digest(inventory), '--extension-command', args.extension_command]
            drivers[name] = {"argv": argv, "sha256": digest(python), "pinned_files": support}
        scenarios.append({"name": scenario, "timeout_seconds": 300,
            "cache_state": 'cold protocol receipt required' if scenario == 'cold_launch' else 'one completed warm-up launch' if scenario == 'warm_launch' else "uncontrolled; hash validation and per-trial copy warm caches",
            "cache_plan_sha256": digest(args.cache_plan) if scenario == 'cold_launch' else None,
            "renderer": args.renderer, "fixtures": [{"path": str(fixture), "sha256": fixture_hash}],
            "comparable_metrics": ['save_to_clean_ack_us'] if scenario == 'save' and len(names) == 2 else [], "drivers": drivers})
    write_new(destination, {"schema_version": 2, "series": "local", "machine_id": args.machine_id,
        "configuration": args.configuration, "repetitions": args.repetitions,
        "qualification_environment": qualification_environment,
        "source_identity": {"algorithm": "t09-run-test-evidence-v1", "root": str(source_root),
                            "excluded_paths": [str(destination)], "before": source_before},
        "applications": applications, "scenarios": scenarios,
        "comparison_review": "Only Save command-to-clean acknowledgement has a shared default endpoint and identical PERF_SAVE edit. This is not a durability/paint comparison. Native presented/whole-find and Scintilla roundtrip endpoints otherwise differ; review identical work and sampling boundaries before listing further comparable_metrics."})


if __name__ == '__main__':
    main()
