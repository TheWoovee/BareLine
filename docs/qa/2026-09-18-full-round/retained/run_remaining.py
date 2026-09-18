from pathlib import Path
import argparse, json, runpy, sys

root = Path.cwd()
capture = runpy.run_path(str(root / '.github/workflows/run_test_evidence.py'))['run_receipt']
evidence = root / 'target/qualification/full-round-20260918'
py = sys.executable
pwsh = ['pwsh', '-NoProfile', '-File']
plans = [
 ('02-metadata', ['cargo','metadata','--locked','--format-version','1'], 120),
 ('03-portability', [py,'.github/workflows/check_portability.py'], 90),
 ('04-qa-dispatch', ['cargo','test','--locked','-p','bareline-file-io','--features','qa-faults','qa_faults'], 300),
 ('05-qa-boundary', ['cargo','test','--locked','-p','bareline','--features','qa-faults','qa_faults'], 600),
 ('06-recovery-probe', ['cargo','test','--locked','-p','bareline','--example','recovery_inspect'], 300),
 ('07-rustfmt', ['cargo','fmt','--all','--check'], 90),
 ('08-toolchain', [py,'.github/workflows/check_toolchain_contract.py','--metadata','target/qualification/full-round-20260918/02-metadata.json.stdout.log'], 60),
 ('09-clippy-census', ['cargo','clippy','--workspace','--all-targets','--locked','--message-format=json'], 900),
 ('10-clippy-ratchet', [py,'.github/workflows/check_clippy_ratchet.py','check','--diagnostics',str(evidence/'09-clippy-census.json.stdout.log'),'--baseline','.github/workflows/clippy-baseline.json','--source-root','.', '--platform','Windows','--target','x86_64-pc-windows-msvc','--toolchain','1.98.1'], 120),
 ('11-e2e-tooling', [py,'-m','unittest','discover','-s','tests/e2e','-p','test*.py'], 300),
 ('12-release-tooling', [py,'-m','unittest','discover','-s','scripts','-p','test*.py'], 300),
 ('13-performance-tooling', [py,'-m','unittest','discover','-s','tests/perf','-p','test*.py'], 180),
 ('14-soak-tooling', [py,'-m','unittest','discover','-s','tests/soak','-p','test*.py'], 90),
 ('15-capture-selftest', [py,'.github/workflows/run_test_evidence.py','--self-test'], 90),
 ('16-clippy-selftest', [py,'.github/workflows/check_clippy_ratchet.py','self-test'], 90),
 ('17-package-local', pwsh+['packaging/windows/test-local.ps1'], 120),
 ('18-visual-oracles', ['powershell','-NoProfile','-File','tests/e2e/test_native_visual.ps1'], 90),
 ('19-runtime-boundary', pwsh+['scripts/check-extension-boundary.ps1'], 90),
 ('20-journey-manifest', [py,'tests/e2e/runner.py','validate'], 60),
 ('21-dependency-policy', ['cargo','deny','--locked','check'], 300),
 ('22-first-party-all', pwsh+['scripts/test-first-party.ps1','-Suite','All'], 2100),
 ('23-render-perf-smoke', ['cargo','xtask','perf','smoke'], 600),
 ('24-multi-gib-diff', ['cargo','test','-p','bareline-diff','--lib','--locked','paged::tests::divergent_multi_gb_full_traversal','--','--exact','--ignored','--nocapture'], 600),
 ('25-release-fixture', pwsh+['packaging/windows/test-release-fixture.ps1'], 1500),
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
