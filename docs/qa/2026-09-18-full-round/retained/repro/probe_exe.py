from pathlib import Path
import os,subprocess,json,hashlib,re,time
r=Path('target/qualification/full-round-20260918/repro/probe-exe').resolve();r.mkdir(exist_ok=True)
vc=Path('C:/Program Files (x86)/Microsoft Visual Studio/2022/BuildTools/VC/Auxiliary/Build/vcvars64.bat')
c=subprocess.run(f'cmd.exe /d /s /c ""{vc}" >nul && set"',capture_output=True,text=True,check=True)
env=dict(os.environ)
for line in c.stdout.splitlines():
 k,sep,v=line.partition('=')
 if sep: env[k]=v
cl=next(Path(p)/'cl.exe' for p in env['Path'].split(';') if (Path(p)/'cl.exe').is_file())
source='#include <typeinfo>\n#include <cstdio>\nnamespace {struct Probe {}; }\nint main() { std::puts(typeid(Probe).raw_name()); }\n'
out=[]
for replica in ('a','b'):
 d=r/replica;d.mkdir(exist_ok=True);p=d/'probe.cpp';p.write_text(source)
 for mode in ('plain','trim'):
  obj=d/(mode+'.exe');args=[str(cl),'/nologo','/EHsc','/MD','/O2','/Brepro',str(p),'/Fe'+str(obj)]
  if mode=='trim': args.append('/d1trimfile:'+str(d)+'\\')
  args.extend(['/link','/Brepro'])
  q=subprocess.run(args,cwd=d,env=env,capture_output=True,text=True,check=True)
  b=obj.read_bytes();names=sorted(set(x.decode() for x in re.findall(rb'\?A0x[0-9a-f]+',b)))
  out.append(dict(replica=replica,mode=mode,sha256=hashlib.sha256(b).hexdigest(),names=names,stdout=q.stdout,stderr=q.stderr))
print(json.dumps(out,indent=2)); (r.parent/'anonymous-namespace-exe-probe.json').write_text(json.dumps(out,indent=2)+'\n')

assert out[1]["sha256"]==out[3]["sha256"],"Trimmed executable replicas must match"
