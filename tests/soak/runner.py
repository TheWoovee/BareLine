# SPDX-License-Identifier: MPL-2.0
"""Bounded core Windows soak. A short run never qualifies the 72-hour release gate."""
import argparse
from dataclasses import asdict
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'tests/perf'))
sys.path.insert(0, str(ROOT / 'tests/e2e'))
import lab_fixture
import evidence_json
spec = importlib.util.spec_from_file_location('soak_evidence', ROOT / '.github/workflows/run_test_evidence.py')
evidence = importlib.util.module_from_spec(spec); spec.loader.exec_module(evidence)
DISK_LIMIT = 512 * 1024 * 1024
SAMPLE_LIMIT = 32 * 1024 * 1024
POOLS = {'task', 'extensions', 'recovery_retirement'}


def digest(path):
    with path.open('rb') as stream: return hashlib.file_digest(stream, 'sha256').hexdigest()


def read_json(path):
    lab_fixture.regular(path)
    with path.open('rb') as stream:raw=stream.read(65537)
    if len(raw)>65536:raise ValueError('Unsafe or excessive observation')
    return evidence_json.loads(raw)


def disk_bytes(root):
    total = count = 0
    for directory, dirs, files in os.walk(root, followlinks=False):
        for name in dirs + files:
            path = Path(directory) / name; info = path.lstat(); count += 1
            if count > 8192 or path.is_symlink() or getattr(info, 'st_file_attributes', 0) & 0x400:
                raise ValueError('Scratch entry quota or reparse boundary exceeded')
            if name in files: total += info.st_size
        if total > DISK_LIMIT: raise ValueError('Scratch exceeds 512 MiB')
    return total


def validate_heartbeat(row, previous, identities):
    owned=dict(identities)
    if row.get('pid') not in owned or not row.get('created_utc') or type(row.get('created_filetime')) is not int or row['created_filetime']!=owned[row['pid']]:
        raise ValueError('Editor identity missing from owned tree')
    if previous and (row['pid'], row['created_utc']) != (previous['pid'], previous['created_utc']):
        raise ValueError('Editor restarted; uninterrupted claim refused')
    for key in ('cycles', 'handles', 'threads', 'private_bytes'):
        if type(row.get(key)) is not int or row[key] < (0 if key == 'cycles' else 1):
            raise ValueError('Missing/nonpositive measurement: ' + key)
    queues = row.get('worker_queues', {})
    if queues.get('pid') != row['pid'] or set(queues.get('pools', {})) != POOLS:
        raise ValueError('Worker queue identity or coverage differs')
    if type(queues.get('sequence')) is not int or queues['sequence'] < 0: raise ValueError('Invalid queue sequence')
    for pool in queues['pools'].values():
        if pool.get('initialized') is False: continue
        if pool.get('initialized') is not True: raise ValueError('Unknown pool initialization state')
        for key in ('workers', 'running', 'queued', 'submitted', 'completed', 'rejected', 'peak_running'):
            if type(pool.get(key)) is not int or pool[key] < 0: raise ValueError('Missing queue counter')
        if not pool['workers'] or pool['running'] > pool['workers']: raise ValueError('Worker bound exceeded')
    if previous and (row['cycles'] < previous['cycles'] or queues['sequence'] < previous['worker_queues']['sequence']):
        raise ValueError('Workload progress moved backwards')


def assess(samples, result, duration, extensions=False, recovery=False):
    errors = []
    if result.get('status') != 'PASS' or result.get('clean_exit') is not True: errors.append('Driver failed or did not exit cleanly')
    if result.get('elapsed_seconds', 0) < duration: errors.append('Requested elapsed duration not reached')
    if len(samples) < 2: errors.append('Fewer than two cadence samples')
    if samples and (result.get('cycles', 0) < 1 or result.get('pid') != samples[-1]['heartbeat']['pid']): errors.append('Missing workload cycles or final PID differs')
    if samples and result.get('created_filetime')!=samples[-1]['heartbeat'].get('created_filetime'):errors.append('Final editor creation identity differs')
    if samples:
        first, last = samples[0]['heartbeat'], samples[-1]['heartbeat']
        # Conservative shakedown alarms, not published performance targets.
        for key, bound in [('handles', 256), ('threads', 32), ('private_bytes', 256 * 1024 * 1024)]:
            if last[key] - first[key] > bound: errors.append(key + ' growth exceeds soak alarm')
    extension_ok=result.get('extension_workload')=='PASS' and result.get('extension_cycles',0)==result.get('cycles') and result.get('cycles',0)>0
    recovery_ok=result.get('recovery_workload')=='PASS' and result.get('recovered_from_pid')==result.get('pid') and result.get('recovery_pid')!=result.get('pid') and type(result.get('recovery_pid')) is int
    if extensions and not extension_ok:errors.append('Requested extension workload incomplete')
    if recovery and not recovery_ok:errors.append('Requested post-soak recovery incomplete or identity differs')
    return {'core_status': 'FAIL' if errors else 'PASS', 'errors': errors,
            'full_release_soak_qualified': False,
            'ready_for_independent_assessment':not errors and duration>=72*3600 and extension_ok and recovery_ok,
            'unverified': ([] if extension_ok else ['Extension-inclusive workload']) + ([] if recovery_ok else ['Crash recovery after soak']) + ['Independent assessment'] + ([] if duration >= 72*3600 else ['72 actual uninterrupted hours'])}


def run(args):
    if os.name != 'nt': raise ValueError('Requires real Windows')
    if not 30 <= args.duration_seconds <= 72*3600 or not 5 <= args.cadence_seconds <= 60: raise ValueError('Duration/cadence outside bounds')
    executable = lab_fixture.regular(args.executable.absolute()).resolve(strict=True)
    if digest(executable) != args.sha256.lower(): raise ValueError('Pinned editor digest differs')
    root = lab_fixture.regular(args.output.absolute()).resolve(); root.mkdir(parents=True, exist_ok=False)
    before = evidence.source_identity(ROOT, (root,))
    if before.get('available') is not True: raise ValueError('Source identity unavailable')
    request = dict(schema_version=1, executable=str(executable), binary_sha256=args.sha256.lower(), scratch=str(root), dpi=args.dpi,
                   duration_seconds=args.duration_seconds, cadence_seconds=args.cadence_seconds)
    bindings=[]
    for key,journey in [('extensions','extension_isolation'),('recovery','crash_recovery')]:
        config=getattr(args,key+'_lab_config',None)
        if config:
            owned=root/(key+'-bindings');owned.mkdir()
            prepared=lab_fixture.prepare(config,journey,owned)
            request[key+'_fixture']=str(owned/'lab-fixture.json')
            bindings.append((config,journey,prepared,digest(owned/'lab-fixture.json')))
    request_path=root/'request.json'; request_path.write_text(json.dumps(request, indent=2), encoding='utf-8')
    from windows_process_metrics import OwnedProcessTree
    powershell = Path(os.environ['SystemRoot'])/'System32/WindowsPowerShell/v1.0/powershell.exe'
    samples=[]; previous=None; failure=None; result={}; start=time.monotonic(); last_cycle_time=start; tree=None
    stdout=root/'samples.jsonl'; last_sample=start-args.cadence_seconds; status=None;post_started=None
    try:
        tree=OwnedProcessTree([str(powershell), '-NoProfile', '-NonInteractive', '-File', str(Path(__file__).with_name('native_soak.ps1')), '-RequestPath', str(request_path)], root)
        with stdout.open('xb') as stream:
            while True:
                status=tree.poll_exit_code()
                if status is not None: break
                now=time.monotonic()
                if (root/'STOP').exists(): raise ValueError('Explicit stop requested')
                if now-start > args.duration_seconds+480: raise ValueError('Soak deadline exceeded')
                if post_started is not None and now-post_started>300:raise ValueError('Post-soak recovery deadline exceeded')
                if post_started is None and now-last_cycle_time > max(120, args.cadence_seconds*3): raise ValueError('Workload heartbeat stalled')
                if now-last_sample >= args.cadence_seconds and (root/'heartbeat.json').exists():
                    heartbeat=read_json(root/'heartbeat.json'); point=tree.sample()
                    if point.missing_processes: raise ValueError('Incomplete process measurement')
                    if heartbeat.get('phase')=='post_recovery':
                        if not request.get('recovery_fixture') or previous is None or (heartbeat['pid'],heartbeat['created_filetime'])!=(previous['pid'],previous['created_filetime']) or heartbeat.get('soak_elapsed_seconds',0)<args.duration_seconds:raise ValueError('Invalid recovery phase transition')
                        if post_started is None:post_started=now
                        disk_bytes(root);last_sample=now
                        time.sleep(0.2);continue
                    if post_started is not None:raise ValueError('Recovery restart cannot re-enter uninterrupted soak')
                    validate_heartbeat(heartbeat, previous, point.identities)
                    if previous is None or heartbeat['cycles'] > previous['cycles']: last_cycle_time=now
                    row=dict(elapsed_seconds=now-start, heartbeat=heartbeat, process_tree=asdict(point), scratch_bytes=disk_bytes(root))
                    raw=(json.dumps(row,separators=(',',':'))+'\n').encode()
                    if stream.tell()+len(raw)>SAMPLE_LIMIT or len(samples)>=52000: raise ValueError('Sample retention quota exceeded')
                    stream.write(raw);stream.flush()
                    samples.append(row);previous=heartbeat;last_sample=now
                time.sleep(0.2)
        result=read_json(root/'driver-result.json')
        if status != 0: raise ValueError('Driver exited unsuccessfully: '+str(status))
    except (ValueError,OSError,RuntimeError,KeyboardInterrupt) as error:
        failure=str(error) or type(error).__name__
    finally:
        if tree: tree.close()
    after=evidence.source_identity(ROOT,(root,))
    if before != after or digest(executable) != args.sha256.lower(): failure='Source or binary changed during run'
    for config,journey,prepared,fixture_digest in bindings:
        try:
            kind='extensions' if journey=='extension_isolation' else 'recovery'
            if digest(Path(request[kind+'_fixture']))!=fixture_digest:raise ValueError('Staged lab configuration changed')
            if lab_fixture.load(config,journey)[1]!=prepared['identity']['lab_config_sha256']:raise ValueError('Lab input changed')
            for asset in prepared['assets']:
                if digest(Path(prepared['paths'][asset['id']]))!=asset['sha256']:raise ValueError('Staged lab asset changed')
        except (OSError,ValueError) as error:failure=str(error)
    assessment=assess(samples,result,args.duration_seconds,bool(request.get('extensions_fixture')),bool(request.get('recovery_fixture')))
    if failure: assessment['errors'].append(failure);assessment['core_status']='FAIL'
    report=dict(schema_version=1, request=request, source_before=before,source_after=after,
                environment=dict(system=platform.system(),release=platform.release(),version=platform.version(),machine=platform.machine()),
                actual_elapsed_seconds=time.monotonic()-start,sample_count=len(samples),driver_result=result,assessment=assessment,
                sample_artifact=dict(path=str(stdout),sha256=digest(stdout)) if stdout.exists() else None)
    (root/'report.json').write_text(json.dumps(report,indent=2),encoding='utf-8')
    print(json.dumps(assessment));return 0 if assessment['core_status']=='PASS' else 1


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--executable',type=Path,required=True);parser.add_argument('--sha256',required=True)
    parser.add_argument('--output',type=Path,required=True);parser.add_argument('--duration-seconds',type=int,default=180)
    parser.add_argument('--cadence-seconds',type=int,default=15);parser.add_argument('--dpi',type=int,default=100)
    parser.add_argument('--extensions-lab-config',type=Path)
    parser.add_argument('--recovery-lab-config',type=Path)
    raise SystemExit(run(parser.parse_args()))
