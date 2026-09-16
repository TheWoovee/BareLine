# SPDX-License-Identifier: MPL-2.0
"""Explicit lab inputs. Validation/copying never executes an asset or asserts PASS."""
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import stat
import evidence_json

JOURNEYS={'crash_recovery','extension_isolation','install_update_rollback'}
REQUIRED={'crash_recovery':{'recovery_probe'},'extension_isolation':{'runtime','catalog'},
          'install_update_rollback':{'installer'}}
LIMIT=256*1024
SAVED=b'\xef\xbb\xbfa\xffz\r\n'
SAVED_TEXT='a\ufffdz\r\n!'
UNTITLED='Untitled durable e\u0301 \U0001f389'

def sha(path):
    with path.open('rb') as stream:return hashlib.file_digest(stream,'sha256').hexdigest()

def regular(path):
    if not path.is_absolute():raise ValueError('Lab paths must be absolute')
    for part in (path,*path.parents):
        try:info=part.lstat()
        except FileNotFoundError:continue
        if stat.S_ISLNK(info.st_mode) or getattr(info,'st_file_attributes',0)&0x400:raise ValueError('Lab reparse path refused')
    return path

def load(path,journey):
    regular(path)
    with path.open('rb') as stream:raw=stream.read(LIMIT+1)
    if len(raw)>LIMIT:raise ValueError('Lab config exceeds bound')
    doc=evidence_json.loads(raw)
    if type(doc.get('schema_version')) is not int or doc['schema_version']!=1 or doc.get('journey')!=journey or journey not in JOURNEYS:raise ValueError('Lab journey/schema mismatch')
    if not re.fullmatch(r'[0-9a-fA-F-]{36}',doc.get('machine_uuid','')) or not doc.get('snapshot_id'):raise ValueError('Explicit disposable VM identity/snapshot required')
    assets=doc.get('assets',[])
    if not 1<=len(assets)<=48:raise ValueError('Lab asset count')
    roles=set();names=set();total=0
    for asset in assets:
        role=asset['id'];name=asset['relative'];relative=PurePosixPath(name)
        if not re.fullmatch(r'[a-z0-9_]{1,64}',role) or role in roles:raise ValueError('Duplicate/invalid lab role')
        if relative.is_absolute() or not relative.parts or any(not re.fullmatch(r'[A-Za-z0-9_.-]+',p) or p in {'.','..'} for p in relative.parts):raise ValueError('Unsafe lab asset destination')
        if name.casefold() in names:raise ValueError('Duplicate lab asset path')
        roles.add(role);names.add(name.casefold());source=regular(Path(asset['path']))
        if not source.is_file() or sha(source)!=asset['sha256']:raise ValueError('Lab asset hash mismatch')
        total+=source.stat().st_size
        if total>256*1024*1024:raise ValueError('Lab asset quota')
    if not REQUIRED[journey]<=roles:raise ValueError('Required lab asset missing')
    if journey=='crash_recovery':
        if doc.get('save_point') not in {'StageFlushed','BeforeReplace','AfterReplace'}:raise ValueError('Unsupported observed save boundary')
    if journey=='extension_isolation':
        packages=doc.get('packages',[])
        if len(packages)!=3 or {p.get('kind') for p in packages}!={'json','xml','hex'}:raise ValueError('Exactly JSON/XML/Hex packages required')
        for p in packages:
            if type(p.get('catalog_index')) is not int or not 0<=p['catalog_index']<=2 or not p.get('installed_label'):raise ValueError('Exact package UI bindings required')
        if not re.fullmatch('[0-9a-f]{64}',doc.get('host_sha256','')):raise ValueError('Pinned host digest required')
    if journey=='install_update_rollback':
        for key in ('publisher_sha256','update_sha256','helper_sha256'):
            if not re.fullmatch('[0-9a-f]{64}',doc.get(key,'')):raise ValueError('Pinned update/publisher/helper required')
        if type(doc.get('activation_failure_exit_code')) is not int or doc['activation_failure_exit_code']==0:raise ValueError('Declared failed activation exit code required')
    return doc,hashlib.sha256(raw).hexdigest()

def prepare(path,journey,scratch):
    doc,digest=load(path,journey);root=scratch/'lab-assets';root.mkdir()
    paths={}
    for asset in doc['assets']:
        target=root/asset['relative'];regular(target);target.parent.mkdir(parents=True,exist_ok=True)
        remaining=Path(asset['path']).stat().st_size
        with Path(asset['path']).open('rb') as source,target.open('xb') as output:
            while chunk:=source.read(min(1024*1024,remaining+1)):
                remaining-=len(chunk)
                if remaining<0:raise ValueError('Lab asset grew during copy')
                output.write(chunk)
        if sha(target)!=asset['sha256']:raise ValueError('Lab asset changed while copying')
        paths[asset['id']]=str(target)
    result=dict(doc,paths=paths,identity=dict(lab_config_sha256=digest,journey=journey),
                saved_hex=SAVED.hex(),saved_text=SAVED_TEXT,untitled=UNTITLED)
    (scratch/'lab-fixture.json').write_text(json.dumps(result,indent=2),encoding='utf-8')
    return result

RECORDS={
 'crash_recovery':[['durable saved and untitled'],['observed save boundary','owned editor terminated','atomic saved bytes'],['recovered saved bytes','recovered untitled bytes']],
 'extension_isolation':[['reviewed installed packages'],['json extension result','xml extension result','hex extension result'],['owned host terminated','revoked extension denied']],
 'install_update_rollback':[['signed installation verified'],['verified update staged','activation failure observed','rollback bytes verified'],['restored editor verified','uninstall inventory verified']]}

def validate_observations(journey,steps,records,fixture=None):
    if not isinstance(records,list):raise ValueError('Lab observations missing')
    for step,required in zip(steps,RECORDS[journey],strict=True):
        if step['status']=='PASS':
            for name in required:
                matches=[r for r in records if r.get('stage')==name]
                if len(matches)!=1 or not isinstance(matches[0].get('details'),dict) or not matches[0]['details']:raise ValueError('Lab observation missing/ambiguous: '+name)
    def detail(name):return next(r['details'] for r in records if r.get('stage')==name)
    def require(ok,message):
        if not ok:raise ValueError(message)
    passed={s['id'] for s in steps if s['status']=='PASS'}
    if journey=='crash_recovery':
        if 's1' in passed:
            expected={hashlib.sha256(v.encode()).hexdigest() for v in (SAVED_TEXT,UNTITLED)}
            rows=detail('durable saved and untitled').get('rows',[])
            require({r.get('recovered',{}).get('sha256') for r in rows}==expected and all(r.get('last_durable') and r.get('status')=='Complete' for r in rows),'Durable copies do not prove both exact documents')
        if 's2' in passed:
            signal=detail('observed save boundary');dead=detail('owned editor terminated')
            require(type(dead.get('pid')) is int and signal.get('pid')==dead['pid'] and signal.get('point')==dead.get('boundary') and re.fullmatch('[0-9a-f]{32}',signal.get('token','')),'Foreign/stale save boundary')
            require(detail('atomic saved bytes').get('sha256') in {hashlib.sha256(v).hexdigest() for v in (SAVED,SAVED+b'!')},'Partial saved bytes')
        if 's3' in passed:
            for name,data in [('saved',SAVED+b'!'),('untitled',UNTITLED.encode())]:
                require(detail('recovered '+name+' bytes').get('sha256')==hashlib.sha256(data).hexdigest(),'Recovered bytes/provenance differ')
    elif journey=='extension_isolation':
        if 's2' in passed:
            require(detail('json extension result').get('text')=='{"n":9007199254740993}','JSON precision/result differs')
            require(detail('xml extension result').get('value')=='Valid XML; external resolution disabled','XML result differs')
            value=detail('hex extension result').get('value','')
            require('FF FE 41 00' in value and 'Unsaved text edits are excluded' in value,'Original Hex bytes/provenance differ')
        if 's3' in passed:
            dead=detail('owned host terminated');denied=detail('revoked extension denied')
            require(type(dead.get('pid')) is int and dead.get('parent_pid')==denied.get('editor_pid') and dead['pid']!=denied.get('editor_pid') and denied.get('host_alive') is False,'Host ownership/revocation differs')
    elif fixture:
        if 's2' in passed:
            require(detail('verified update staged').get('sha256')==fixture['update_sha256'],'Staged update differs')
            require(detail('activation failure observed').get('expected_exit_code')==fixture['activation_failure_exit_code'],'Failed activation differs')
