# SPDX-License-Identifier: MPL-2.0
"""Rolling-baseline selection and staleness for the nightly regression job.

Never fabricates history. Fewer than the required comparable baselines is a warning,
not a silent skip; the candidate's own artifact is retained as a future baseline.
A newest baseline older than the staleness limit is reported so the workflow can
alert. Reports whose cohort differs (machine, configuration, fixtures, settings or
source identity) are excluded with their reason instead of failing the comparison.
"""
import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import sys

import perf_suite

REQUIRED_BASELINES = 7


def parse_time(value):
    parsed = datetime.fromisoformat(str(value).replace('Z', '+00:00'))
    return parsed if parsed.tzinfo else parsed.replace(tzinfo=timezone.utc)


def find_report(directory, name):
    matches = sorted(Path(directory).rglob(name)) if Path(directory).is_dir() else []
    return matches[0] if matches else None


def evaluate(candidate, history, now, max_age_days, required=REQUIRED_BASELINES):
    """`history`: previous runs newest first, each {run_id, created_at, report, path}.

    `report` is the parsed series report or None when its artifact is absent.
    """
    selected, excluded = [], []
    newest = None
    for run in sorted(history, key=lambda item: parse_time(item['created_at']), reverse=True):
        if run.get('report') is None:
            excluded.append({'run_id': run['run_id'], 'reason': 'report artifact is absent or expired'})
            continue
        if newest is None:
            newest = run
        reason = perf_suite.cohort_mismatch(candidate, run['report']) if candidate else 'no candidate report'
        if reason:
            excluded.append({'run_id': run['run_id'], 'reason': reason})
        elif len(selected) < required:
            selected.append(run)
    age_days = (now - parse_time(newest['created_at'])).total_seconds() / 86400 if newest else None
    stale = age_days is not None and age_days > max_age_days
    candidate_valid = bool(candidate) and perf_suite.report_source_valid(candidate)
    ready = candidate_valid and len(selected) == required
    annotations = []
    if not candidate:
        annotations.append('::error title=Performance history::the current run produced no report to compare')
    elif not candidate_valid:
        annotations.append('::warning title=Performance history::the current report is unqualified (source identity '
                           'unavailable or changed); it is recorded but not compared')
    if candidate and len(selected) < required:
        annotations.append(f'::warning title=Performance history::only {len(selected)} of {required} comparable '
                           'baselines exist; this run is recorded and the rolling comparison starts once '
                           f'{required} are available')
    if stale:
        annotations.append(f'::error title=Stale performance baseline::the newest baseline (run {newest["run_id"]}) '
                           f'is {age_days:.1f} days old; the limit is {max_age_days} days')
    return {'schema_version': 1, 'required': required, 'candidate_present': bool(candidate),
            'candidate_valid': candidate_valid, 'ready': ready, 'stale': stale,
            'max_age_days': max_age_days,
            'newest_baseline': None if newest is None else {
                'run_id': newest['run_id'], 'created_at': newest['created_at'], 'age_days': round(age_days, 2)},
            'selected': [{'run_id': run['run_id'], 'created_at': run['created_at'], 'path': str(run['path'])}
                         for run in selected],
            'excluded': excluded, 'annotations': annotations}


def render(result, series):
    lines = [f'## Baseline history ({series})', '']
    newest = result['newest_baseline']
    lines.append(f'- Comparable baselines: {len(result["selected"])} of {result["required"]}'
                 + ('' if result['ready'] else ' (rolling comparison not run; this run is recorded)'))
    lines.append('- Newest baseline: ' + ('none' if newest is None else
                 f'run {newest["run_id"]} at {newest["created_at"]}, {newest["age_days"]} days old'
                 + (f' - **stale** (limit {result["max_age_days"]} days)' if result['stale'] else '')))
    for item in result['excluded'][:20]:
        lines.append(f'- Excluded run {item["run_id"]}: {item["reason"]}')
    return '\n'.join(lines) + '\n'


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--candidate', default='', help='current report; empty when the run produced none')
    parser.add_argument('--runs', required=True, help='JSON list of {databaseId, createdAt} newest first')
    parser.add_argument('--history-dir', required=True, help='directory with one subdirectory per run id')
    parser.add_argument('--report-name', required=True)
    parser.add_argument('--series', required=True)
    parser.add_argument('--max-age-days', type=float, default=3)
    parser.add_argument('--now', help='ISO timestamp; defaults to the current UTC time')
    parser.add_argument('--output', required=True, help='selection JSON (create-new)')
    parser.add_argument('--summary', help='append the Markdown status here, e.g. $GITHUB_STEP_SUMMARY')
    parser.add_argument('--github-output', help='append ready/stale outputs here, e.g. $GITHUB_OUTPUT')
    args = parser.parse_args(argv)
    candidate = perf_suite.read_json(args.candidate) if args.candidate and Path(args.candidate).is_file() else None
    with open(args.runs, encoding='utf-8-sig') as stream:
        runs = json.load(stream) or []
    history = []
    for run in runs:
        path = find_report(Path(args.history_dir) / str(run['databaseId']), args.report_name)
        history.append({'run_id': run['databaseId'], 'created_at': run['createdAt'], 'path': path,
                        'report': perf_suite.read_json(path) if path else None})
    now = parse_time(args.now) if args.now else datetime.now(timezone.utc)
    result = evaluate(candidate, history, now, args.max_age_days)
    perf_suite.write_new(args.output, result)
    for annotation in result['annotations']:
        print(annotation)
    if args.summary:
        with open(args.summary, 'a', encoding='utf-8', newline='\n') as stream:
            stream.write(render(result, args.series))
    if args.github_output:
        with open(args.github_output, 'a', encoding='utf-8', newline='\n') as stream:
            stream.write(f'ready={str(result["ready"]).lower()}\nstale={str(result["stale"]).lower()}\n')
    return 0 if candidate else 1


if __name__ == '__main__':
    sys.exit(main())
