"""Rasterize vector icons with resvg-py==0.5.0 and assemble with Pillow==12.3.0.

Install: python -m pip install resvg-py==0.5.0 Pillow==12.3.0
Run: python scripts/render_icons.py
The ICO remains a draft until human review of the native 16 px image.
"""
from pathlib import Path
import io,json,re,shutil,struct
import resvg_py
from PIL import Image

ROOT=Path(__file__).resolve().parents[1]
OUT=ROOT/'mockups/logo-brand'
SIZES=(16,20,24,32,40,48,64,128,256)

def preserve(path,data):
    if path.exists() and path.read_bytes()==data:
        return None
    prior=None
    if path.exists():
        archive=(ROOT/'mockups/superseded').resolve()
        path.resolve().relative_to(ROOT.resolve())
        n=1
        while (archive/f'{path.stem}-v{n}{path.suffix}').exists(): n+=1
        target=(archive/f'{path.stem}-v{n}{path.suffix}').resolve()
        target.relative_to(archive)
        shutil.move(str(path),str(target))
        prior=target.relative_to(ROOT).as_posix()
    path.write_bytes(data)
    return prior

def render():
    OUT.mkdir(exist_ok=True)
    logpath=ROOT/'mockups/GENERATION_LOG.json'
    log=json.loads(logpath.read_text(encoding='utf-8'))
    frames=[]
    for size in SIZES:
        source=OUT/('bareline-mark-16.svg' if size<=20 else 'bareline-mark-dark.svg')
        svg=source.read_text(encoding='utf-8')
        inside=re.search(r'<svg[^>]*>(.*)</svg>',svg,re.S)[1]
        tile='<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"><rect width="24" height="24" rx="5.28" fill="#181B1F"/><g transform="translate(3.36 3.36) scale(.72)">'+inside+'</g></svg>'
        data=resvg_py.svg_to_bytes(svg_string=tile,width=size,height=size)
        path=OUT/f'bareline-icon-{size}.png'
        prior=preserve(path,data)
        frames.append(Image.open(io.BytesIO(data)).convert('RGBA'))
        entry={'type':'image','id':f'vector-icon-{size}','path':path.relative_to(ROOT).as_posix(),'file':path.name,'width':size,'height':size,'date':'2026-09-05','tool':'resvg-py 0.5.0; hand-authored SVG','prompt_file':'scripts/render_icons.py','source_file':source.relative_to(ROOT).as_posix(),'supersedes':prior,'status':'draft; human 16 px review pending'}
        found=next((a for a in log if a.get('id')==entry['id']),None)
        if found:
            if prior:
                historical=dict(found,path=prior,id=found['id']+'-'+Path(prior).stem,status='superseded')
                log.append(historical);log.remove(found);log.append(entry)
        else:log.append(entry)
    # Assemble the exact independently rasterized frames, including 20 and 40 px.
    chunks=[]
    for im in frames:
        buf=io.BytesIO();im.save(buf,format='PNG');chunks.append(buf.getvalue())
    offset=6+16*len(SIZES)
    directory=bytearray(struct.pack('<HHH',0,1,len(SIZES)))
    for size,data in zip(SIZES,chunks):
        directory.extend(struct.pack('<BBBBHHII',size if size<256 else 0,size if size<256 else 0,0,0,1,32,len(data),offset));offset+=len(data)
    ico=OUT/'bareline.ico';prior=preserve(ico,bytes(directory)+b''.join(chunks))
    if prior:log.append({'type':'supersession','path':ico.relative_to(ROOT).as_posix(),'supersedes':prior,'date':'2026-09-05','tool':'Pillow 12.3.0; ICO directory assembly'})
    payload=(json.dumps(log,indent=2,ensure_ascii=False)+'\n').encode()
    old=preserve(logpath,payload)
    if old:
        log.append({'type':'document','path':'mockups/GENERATION_LOG.json','supersedes':old,'date':'2026-09-05','tool':'render_icons.py'})
        logpath.write_text(json.dumps(log,indent=2,ensure_ascii=False)+'\n',encoding='utf-8')
    print('Rendered 9 PNG sizes and draft bareline.ico; human 16 px review pending.')

if __name__=='__main__':render()
