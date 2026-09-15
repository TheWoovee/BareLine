import json
import sys
from pathlib import Path

before = json.loads(Path(sys.argv[1]).read_text(encoding='utf-8-sig'))
after = json.loads(Path(sys.argv[2]).read_text(encoding='utf-8-sig'))
delta = []
for case in sorted(before.keys() | after.keys()):
    old, new = before.get(case, {}), after.get(case, {})
    if old == new:
        continue
    row = {'case': case, 'fields': [], 'added': [], 'removed': [], 'nodes': []}
    for key in old.keys() | new.keys():
        if key != 'nodes' and old.get(key) != new.get(key):
            row['fields'].append({'field': key, 'before': old.get(key), 'after': new.get(key)})
    a = {node['id']: node for node in old.get('nodes', [])}
    b = {node['id']: node for node in new.get('nodes', [])}
    row['added'] = [b[key] for key in b.keys() - a.keys()]
    row['removed'] = [a[key] for key in a.keys() - b.keys()]
    for key in a.keys() & b.keys():
        fields = {name: [a[key].get(name), b[key].get(name)] for name in a[key].keys() | b[key].keys() if a[key].get(name) != b[key].get(name)}
        if fields:
            row['nodes'].append({'id': key, 'fields': fields})
    delta.append(row)
Path(sys.argv[3]).write_text(json.dumps(delta, indent=2, ensure_ascii=False), encoding='utf-8')
for row in delta:
    print(json.dumps(row, ensure_ascii=True))
print(f'{len(delta)} changed cases of {len(after)}; complete structured delta retained')
