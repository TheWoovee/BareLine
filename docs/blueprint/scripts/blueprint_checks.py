"""Shared documentation checks; report exact path/line matches."""
import re

SETTINGS_DEFAULTS={
    'document.resident_max_bytes':'256 MiB',
    'document.page_size_bytes':'1 MiB',
    'document.page_cache_bytes':'64 MiB',
    'document.aggregate_cache_bytes':'256 MiB',
    'undo.aggregate_ram_bytes':'128 MiB',
    'search.results_ram_bytes':'64 MiB',
    'transcode.temp_quota_bytes':'min(20 GiB, 20% free)',
    'clipboard.history.max_entries':'20',
    'clipboard.history.max_total_bytes':'16 MiB',
    'clipboard.history.max_entry_bytes':'4 MiB',
}

def regression_hits(root,paths):
    # Split prohibited literals so the scanner does not report its own definitions.
    forbidden=['sur'+'rogate','escape '+'scheme','escaped'+'_byte','always '+'UTF-8',
               'zero runtime '+'on disk','does not complete '+'comparably',
               'one actor per open document '+'running on the document task']
    terms=re.compile('|'.join(re.escape(x) for x in forbidden),re.I)
    pin_word='pin'+'ned';undo_word='un'+'do'
    retention=re.compile(rf'\b{pin_word}\b[^\n]*\b{undo_word}\b|\b{undo_word}\b[^\n]*\b{pin_word}\b',re.I)
    enable='explicitly '+'enable';strip='tool'+'bar';greeting='Wel'+'come'
    hits=[]
    for path in paths:
        rel=path.relative_to(root).as_posix()
        if 'superseded' in path.parts or path.name.startswith('REVIEW'):continue
        if path.suffix not in {'.md','.txt','.json','.py','.toml','.svg'}:continue
        body=path.read_text(encoding='utf-8-sig')
        for line,text in enumerate(body.splitlines(),1):
            bad=bool(terms.search(text) or retention.search(text))
            if enable.lower() in text.lower() and strip.lower() in text.lower():bad=True
            if (path.name in {'01_REQUIREMENTS.md','11_UI_INTERACTION_SPEC.md','08_IMAGEN_PROMPTS.md'} or rel.startswith('mockups/prompts/')) and re.search(greeting,text,re.I):bad=True
            if bad:hits.append(f'{rel}:{line}:{text}')
    return hits

def prompt_hits(root):
    hits=[]
    for path in [root/'08_IMAGEN_PROMPTS.md',*(root/'mockups/prompts').glob('*.txt')]:
        if path.stem.startswith('bareline-toolbar-enabled'):continue
        for number,line in enumerate(path.read_text(encoding='utf-8').splitlines(),1):
            if re.search('tool'+'bar',line,re.I):hits.append(f'{path.relative_to(root).as_posix()}:{number}:{line}')
    return hits

def settings_table_errors(text):
    errors=[]
    for key,default in SETTINGS_DEFAULTS.items():
        if f'| `{key}` | {default} |' not in text:errors.append(key)
    return errors

if __name__=='__main__':
    from pathlib import Path
    import sys
    root=Path(__file__).resolve().parents[1]
    paths=[p for p in root.rglob('*') if p.is_file() and not any(part in {'graft','.git','__pycache__','.cache'} for part in p.relative_to(root).parts)]
    matches=regression_hits(root,paths)
    print(f'Regression grep: {len(matches)} hits')
    for match in matches:print(match)
    print('Scope: package text; exclude superseded archives and REVIEW files; apply greeting restriction to requirements, UI spec and prompts.')
    sys.exit(bool(matches))
