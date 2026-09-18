from pathlib import Path
import argparse, json, runpy, sys

root = Path.cwd()
capture = runpy.run_path(str(root / '.github/workflows/run_test_evidence.py'))['run_receipt']
evidence = root / 'target/qualification/full-round-20260918'
py = sys.executable
pwsh = ['pwsh', '-NoProfile', '-File']
plans = [
 ('31-final-quality', pwsh+['target/qualification/full-round-20260918/quality.ps1'], 120),
 ('30-final-workspace', ['cargo','test','--workspace','--locked'], 900),
 ('32-final-clippy', pwsh+['target/qualification/full-round-20260918/clippy.ps1'], 900),
]
parser = argparse.ArgumentParser()
parser.add_argument('--start', default=plans[0][0])
parser.add_argument('--end', default=plans[-1][0])
args = parser.parse_args()
names = [p[0] for p in plans]
for name, command, timeout in plans[names.index(args.start):names.index(args.end)+1]:
 print('START '+name, flush=True)
 result = capture(evidence/(name+'.json'), command, root, timeout)
 print(json.dumps({'name':name,'exit_code':result['exit_code'],'status':result['status'],'elapsed_ms':result['elapsed_ms'],'source_changed':result['source_changed_during_run']}),flush=True)
 if result['top_level_failed'] or result['source_changed_during_run']:
  for stream in ('stdout','stderr'):
   print(Path(result[stream]['path']).read_text(encoding='utf-8',errors='replace')[-6000:],flush=True)
  raise SystemExit(1)
print('SELECTED CHECKS COMPLETE',flush=True)
