# SPDX-License-Identifier: MPL-2.0
"""Markdown and JSON reporting for performance evidence. Never launches an editor.

`comparison` aggregates raw trials written by compare/Invoke-Bench.ps1 into medians
per cache state and configuration. `summary` renders a perf_suite-style series report
(hosted or native nightly) and `regressions` renders a regress() result; both emit
GitHub annotations so a workflow can publish them as a job summary.
A metric that was not measured is shown as n/a, never as zero.
"""
import argparse
import json
import math
from pathlib import Path
import statistics
import sys

import perf_suite

FC10_RUNS = 10
FC10_CACHE_STATES = ('cold', 'warm')
# (scenario, metric, label, unit). Order is the table order. Every metric here is an
# endpoint both drivers observe the same way unless `ONE_SIDED` names it.
METRICS = (
    ('launch', 'window_visible_ms', 'Process start to main window visible', 'ms'),
    ('launch', 'input_idle_ms', 'Process start to input idle', 'ms'),
    ('launch', 'first_frame_ms', 'Process start to first painted frame', 'ms'),
    ('launch', 'ready_ms', 'Process start to ready (visible, idle, painted)', 'ms'),
    ('launch', 'idle_private_mb', 'Idle private bytes after the idle wait', 'MB'),
    ('launch', 'idle_working_set_mb', 'Idle working set after the idle wait', 'MB'),
    ('launch', 'idle_threads', 'Idle threads after the idle wait', 'count'),
    ('launch', 'idle_handles', 'Idle handles after the idle wait', 'count'),
    ('open', 'first_view_ms', 'Open to first interactive viewport', 'ms'),
    ('open', 'loaded_ms', 'Open to fully loaded', 'ms'),
    ('open', 'max_ui_latency_ms', 'Worst UI message round trip while loading', 'ms'),
    ('open', 'loaded_private_mb', 'Private bytes when loaded', 'MB'),
    ('open', 'loaded_peak_working_set_mb', 'Peak working set when loaded', 'MB'),
    ('save', 'save_to_disk_ms', 'Save after a 1-byte edit to bytes on disk', 'ms'),
    ('save', 'save_to_clean_ms', 'Save after a 1-byte edit to clean state', 'ms'),
    ('copy', 'copy_ms', 'Select All + Copy to clipboard populated', 'ms'),
    ('replace', 'replace_all_ms', 'Replace All to document updated', 'ms'),
)
ONE_SIDED = {'first_frame_ms': 'Notepad++ first paint needs ETW; not measured by this harness'}
APPLICATION_ORDER = ('notepadpp', 'bareline')
SCENARIO_ORDER = ('launch', 'open', 'save', 'copy', 'replace')


def number(value):
    """A finite non-negative metric, or None for anything else (never coerced to zero)."""
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    return value if math.isfinite(value) and value >= 0 else None


def read_raw(path):
    path = Path(path)
    raw = perf_suite.read_json(path)
    if raw.get('kind') != 'bareline_notepadpp_comparison' or raw.get('schema_version') != 1:
        raise ValueError('not a version-1 Bareline/Notepad++ comparison raw file')
    if not isinstance(raw.get('trials'), list):
        raise ValueError('raw comparison has no trial list')
    return raw


def config_sort_key(config, applications):
    application = applications.get(config, '')
    rank = APPLICATION_ORDER.index(application) if application in APPLICATION_ORDER else len(APPLICATION_ORDER)
    return rank, config


def reference_config(configs, applications):
    """ADR-10 compares against Notepad++ with plugins disabled when that series exists."""
    comparators = [c for c in configs if applications.get(c) == 'notepadpp']
    for preferred in ('npp-noPlugin', 'npp-default'):
        if preferred in comparators:
            return preferred
    return comparators[0] if comparators else None


def aggregate(raw):
    """Median cells per (scenario, fixture, cache, config, metric) plus failure coverage."""
    trials = raw['trials']
    applications = {}
    cells = {}
    failures = []
    for trial in trials:
        key = (trial.get('scenario'), trial.get('fixture') or '', trial.get('cache'), trial.get('config'))
        if not all(isinstance(part, str) for part in key):
            raise ValueError('trial identity is incomplete: ' + json.dumps(trial.get('id')))
        applications[trial['config']] = trial.get('application')
        if trial.get('status') != 'ok':
            failures.append({k: trial.get(k) for k in ('id', 'scenario', 'fixture', 'cache', 'config',
                                                        'run', 'status', 'error')})
        for metric, value in (trial.get('metrics') or {}).items():
            # A failed trial still creates the cell, so its column shows n/a instead of vanishing.
            cell = cells.setdefault(key + (metric,), {'values': []})
            if trial.get('status') != 'ok':
                continue
            checked = number(value)
            if checked is None:
                raise ValueError(f'invalid persisted metric {metric} in trial {trial.get("id")}')
            cell['values'].append(checked)
    attempted = {}
    for trial in trials:
        key = (trial['scenario'], trial.get('fixture') or '', trial['cache'], trial['config'])
        attempted[key] = attempted.get(key, 0) + 1
    rows = []
    for (scenario, fixture, cache, config, metric), cell in sorted(cells.items()):
        values = cell['values']
        rows.append({'scenario': scenario, 'fixture': fixture or None, 'cache': cache, 'config': config,
                     'application': applications.get(config), 'metric': metric,
                     'samples': len(values), 'attempted': attempted[(scenario, fixture, cache, config)],
                     'median': statistics.median(values) if values else None,
                     'min': min(values) if values else None, 'max': max(values) if values else None})
    return rows, failures, applications


def qualification(raw, rows, failures):
    protocol = raw.get('protocol') or {}
    reasons = []
    runs = protocol.get('runs')
    if not isinstance(runs, int) or runs < FC10_RUNS:
        reasons.append(f'fewer than {FC10_RUNS} measured runs per cell (runs={runs})')
    states = set(protocol.get('cache_states') or ())
    missing = [state for state in FC10_CACHE_STATES if state not in states]
    if missing:
        reasons.append('cache states not measured: ' + ', '.join(missing))
    if 'cold' in states and not protocol.get('cold_cache_command'):
        reasons.append('cold runs have no pinned cache preparation command')
    if protocol.get('reference_machine') is not True:
        reasons.append('not run on the declared reference machine')
    if failures:
        reasons.append(f'{len(failures)} trial(s) failed or timed out')
    short = [r for r in rows if r['samples'] < (runs if isinstance(runs, int) else FC10_RUNS)]
    if short:
        reasons.append(f'{len(short)} cell(s) have fewer samples than runs')
    return {'fc10_qualified': not reasons, 'reasons': reasons,
            'label': 'FC-10 qualified' if not reasons else 'Indicative (not FC-10)'}


def ratios(rows, applications):
    """Bareline median / reference Notepad++ median for every comparable metric."""
    index = {(r['scenario'], r['fixture'], r['cache'], r['metric'], r['config']): r for r in rows}
    result = []
    groups = sorted({(r['scenario'], r['fixture'], r['cache'], r['metric']) for r in rows}, key=str)
    for scenario, fixture, cache, metric in groups:
        if metric in ONE_SIDED:
            continue
        configs = [c for (s, f, k, m, c) in index if (s, f, k, m) == (scenario, fixture, cache, metric)]
        reference = reference_config(configs, applications)
        denominator = index[(scenario, fixture, cache, metric, reference)]['median'] if reference else None
        for config in configs:
            if applications.get(config) != 'bareline':
                continue
            numerator = index[(scenario, fixture, cache, metric, config)]['median']
            result.append({'scenario': scenario, 'fixture': fixture, 'cache': cache, 'metric': metric,
                           'config': config, 'reference': reference,
                           'ratio': numerator / denominator if numerator is not None and denominator else None})
    return result


def format_value(value, unit):
    if value is None:
        return 'n/a'
    if unit == 'ms':
        return f'{value:,.0f} ms'
    if unit == 'MB':
        return f'{value:,.1f} MB'
    return f'{value:,.0f}'


def cell_text(row, unit, attempted=0):
    if row is None:
        return f'n/a [0/{attempted}]' if attempted else 'n/a'
    text = format_value(row['median'], unit)
    if row['samples'] > 1 and row['min'] != row['max']:
        text += f' ({format_value(row["min"], unit)}-{format_value(row["max"], unit)})'
    if row['samples'] != row['attempted']:
        text += f' [{row["samples"]}/{row["attempted"]}]'
    return text


def escape(text):
    return str(text).replace('|', '\\|').replace('\n', ' ')


def render_comparison(raw, rows, failures, applications, qualified, ratio_rows):
    protocol = raw.get('protocol') or {}
    apps = raw.get('applications') or {}
    lines = [f'# Bareline vs Notepad++: {qualified["label"]}', '']
    for name, label in (('bareline', 'Bareline'), ('notepadpp', 'Notepad++')):
        app = apps.get(name) or {}
        lines.append(f'- **{label}:** version `{app.get("version")}`, SHA-256 `{app.get("sha256")}`')
    machine = raw.get('machine') or {}
    lines.append(f'- **Machine:** {escape(machine.get("label"))}; {escape(machine.get("cpu"))}; '
                 f'{escape(machine.get("os"))}')
    lines.append(f'- **Protocol:** {protocol.get("runs")} measured runs per cell, cache states '
                 f'{", ".join(protocol.get("cache_states") or []) or "none"}, idle wait '
                 f'{protocol.get("idle_seconds")} s, medians (min-max) [samples/attempted when short]')
    if qualified['reasons']:
        lines.append('- **Not FC-10 because:** ' + '; '.join(qualified['reasons']))
    lines.append('')
    ratio_index = {(r['scenario'], r['fixture'], r['cache'], r['metric'], r['config']): r['ratio'] for r in ratio_rows}
    by_key = {(r['scenario'], r['fixture'] or '', r['cache'], r['config'], r['metric']): r for r in rows}
    attempts = {}
    for trial in raw['trials']:
        key = (trial['scenario'], trial.get('fixture') or '', trial['cache'], trial['config'])
        attempts[key] = attempts.get(key, 0) + 1
    sections = sorted({(r['scenario'], r['fixture'] or '', r['cache']) for r in rows},
                      key=lambda k: (SCENARIO_ORDER.index(k[0]) if k[0] in SCENARIO_ORDER else len(SCENARIO_ORDER), k))
    known = {m[1] for m in METRICS}
    for scenario, fixture, cache in sections:
        # Configurations whose every trial failed still get a column (n/a [0/N]).
        configs = sorted({c for (s, f, k, c) in attempts if (s, f, k) == (scenario, fixture, cache)},
                         key=lambda c: config_sort_key(c, applications))
        blconfigs = [c for c in configs if applications.get(c) == 'bareline']
        reference = reference_config(configs, applications)
        title = f'## {scenario}' + (f' `{fixture}`' if fixture else '') + f' ({cache} cache)'
        lines += [title, '']
        header = ['Metric', *configs] + [f'{c} / {reference}' for c in blconfigs if reference]
        lines.append('| ' + ' | '.join(header) + ' |')
        lines.append('|' + '---|' + '---:|' * (len(header) - 1))
        metrics = [m for m in METRICS if m[0] == scenario]
        extra = sorted({r['metric'] for r in rows if (r['scenario'], r['fixture'] or '', r['cache'])
                        == (scenario, fixture, cache) and r['metric'] not in known})
        metrics += [(scenario, name, name, '') for name in extra]
        for _, metric, label, unit in metrics:
            present = [by_key.get((scenario, fixture, cache, c, metric)) for c in configs]
            if not any(present):
                continue
            cells = [cell_text(row, unit, attempts.get((scenario, fixture, cache, c), 0))
                     for row, c in zip(present, configs)]
            for config in blconfigs:
                if not reference:
                    continue
                value = ratio_index.get((scenario, fixture or None, cache, metric, config))
                cells.append('n/a' if value is None else f'{value:.2f}x')
            suffix = f' ({ONE_SIDED[metric]})' if metric in ONE_SIDED else ''
            lines.append('| ' + ' | '.join([escape(label + suffix), *cells]) + ' |')
        lines.append('')
    checks = raw.get('checks') or []
    if checks:
        lines += ['## Correctness checks', '', '| Check | Application | Result | Detail |', '|---|---|---|---|']
        for check in checks:
            result = {True: 'pass', False: 'FAIL'}.get(check.get('passed'), 'recorded')
            lines.append('| ' + ' | '.join(escape(check.get(k)) for k in ('name', 'application'))
                         + f' | {result} | {escape(check.get("detail"))} |')
        lines.append('')
    if failures:
        lines += ['## Failed or timed-out trials', '', '| Trial | Status | Error |', '|---|---|---|']
        for failure in failures:
            lines.append(f'| {escape(failure["id"])} | {escape(failure["status"])} | {escape(failure.get("error"))} |')
        lines.append('')
    lines.append('Ratios are Bareline median / Notepad++ median; below 1.00x favours Bareline. '
                 'No comparative claim is eligible without owner review of the full raw evidence.')
    return '\n'.join(lines) + '\n'


def comparison(raw_path, markdown_path, summary_path):
    raw = read_raw(raw_path)
    rows, failures, applications = aggregate(raw)
    qualified = qualification(raw, rows, failures)
    ratio_rows = ratios(rows, applications)
    markdown = render_comparison(raw, rows, failures, applications, qualified, ratio_rows)
    with open(markdown_path, 'x', encoding='utf-8', newline='\n') as stream:
        stream.write(markdown)
    perf_suite.write_new(summary_path, {
        'schema_version': 1, 'kind': 'bareline_notepadpp_comparison_summary',
        'raw': str(raw_path), 'raw_sha256': perf_suite.digest(raw_path),
        'qualification': qualified, 'statistic': 'median of successful runs; min/max retained',
        'rows': rows, 'ratios': ratio_rows, 'failures': failures,
        'checks': raw.get('checks') or [], 'claims_eligible': False})
    return markdown


def render_series(report, title):
    """Markdown table for a perf_suite-shaped report (hosted legacy or native nightly)."""
    rows = report.get('rows') or []
    lines = [f'## {title}', '']
    manifest = (report.get('provenance') or {}).get('manifest') or {}
    lines.append(f'Series `{manifest.get("series")}` on `{manifest.get("machine_id")}`, commit '
                 f'`{manifest.get("commit")}`. Informational; not comparative evidence.')
    lines.append('')
    if not rows:
        lines.append('No complete rows were recorded in this run.')
    else:
        lines += ['| Scenario | Metric | Samples | Bareline P50 | Bareline P95 | Notepad++ P50 | Status |',
                  '|---|---|---:|---:|---:|---:|---|']
        for row in rows:
            bareline = row.get('bareline') or {}
            comparator = row.get('notepadpp') or {}
            samples = row.get('paired_samples', row.get('sample_count'))
            cells = [row.get('scenario'), row.get('metric'), samples,
                     *(('n/a' if number(v) is None else f'{v:,}') for v in
                       (bareline.get('p50'), bareline.get('p95'), comparator.get('p50'))),
                     row.get('status', 'n/a')]
            lines.append('| ' + ' | '.join(escape(c) for c in cells) + ' |')
    for failure in report.get('failures') or []:
        lines.append(f'\n- Failure: `{escape(json.dumps(failure, sort_keys=True)[:300])}`')
    return '\n'.join(lines) + '\n'


def render_regressions(result, series):
    """Markdown plus ::warning annotations; numeric findings never fail the job."""
    findings = result.get('findings') or []
    lines = [f'## Rolling regressions ({series})', '']
    annotations = []
    if not findings:
        lines.append('No P50 regression beyond 10% and observed baseline noise.')
    else:
        lines += ['| Scenario | Metric | Baseline P50 | Current P50 | Increase | Tolerance |',
                  '|---|---|---:|---:|---:|---:|']
        for f in findings:
            lines.append(f'| {escape(f["scenario"])} | {escape(f["metric"])} | {f["baseline_p50"]:,} | '
                         f'{f["current_p50"]:,} | {f["increase_fraction"]:.1%} | {f["noise_tolerance_fraction"]:.1%} |')
            annotations.append(f'::warning title=Performance regression ({series})::{f["scenario"]} '
                               f'{f["metric"]} P50 {f["baseline_p50"]:,} -> {f["current_p50"]:,} '
                               f'(+{f["increase_fraction"]:.1%})')
    return '\n'.join(lines) + '\n', annotations


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    compared = sub.add_parser('comparison', help='raw.json -> results.md + summary.json')
    compared.add_argument('raw')
    compared.add_argument('--markdown', required=True)
    compared.add_argument('--json', required=True)
    series = sub.add_parser('summary', help='series report -> Markdown')
    series.add_argument('report')
    series.add_argument('--title', default='Performance series')
    regressed = sub.add_parser('regressions', help='regress() output -> Markdown; annotations on stdout')
    regressed.add_argument('result')
    regressed.add_argument('--series', required=True)
    for command in (series, regressed):
        command.add_argument('--append', help='append the Markdown here (e.g. $GITHUB_STEP_SUMMARY) '
                                              'instead of writing it to stdout')
    args = parser.parse_args(argv)
    if args.command == 'comparison':
        comparison(Path(args.raw), Path(args.markdown), Path(args.json))
        return 0
    annotations = []
    if args.command == 'summary':
        markdown = render_series(perf_suite.read_json(args.report), args.title)
    else:
        markdown, annotations = render_regressions(perf_suite.read_json(args.result), args.series)
    # Workflow commands are only parsed from stdout, so they never go into the summary file.
    for annotation in annotations:
        print(annotation)
    if args.append:
        with open(args.append, 'a', encoding='utf-8', newline='\n') as stream:
            stream.write(markdown)
    else:
        sys.stdout.write(markdown)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
