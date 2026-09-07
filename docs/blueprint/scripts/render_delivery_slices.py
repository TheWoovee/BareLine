"""Render the three per-PR delivery exits from revision_cases.json."""
from pathlib import Path
import json,re
ROOT=Path(__file__).resolve().parents[1]

def delivery_table(slices):
    assert len(slices)==3
    return '| Slice name | What compiles and is testable at slice end | AC cases or minimum scenarios that prove it |\n|---|---|---|\n' + ''.join(
        f"| {s['name']} | {s['exit']} | {s['proof']} |\n" for s in slices)

def render():
    for number,contracts,requirements,summary,cases,slices in json.loads((ROOT/'scripts/revision_cases.json').read_text(encoding='utf-8')):
        path=next((ROOT/'PRs').glob(f'PR-{number:03}_*.md'))
        text=path.read_text(encoding='utf-8')
        text=re.sub(r'## v1\.[23] (?:review resolutions and delivery slices|delivery slices and acceptance)', '## v1.3 delivery slices and acceptance',text)
        head,tail=text.split('## v1.3 delivery slices and acceptance',1)
        tail=re.sub(r'\*\*Normative contracts:\*\* .*? in ',f'**Normative contracts:** {contracts} in ',tail,count=1)
        tail=re.sub(r'Deliver independently reviewable increments.*?(?=\n\n)',delivery_table(slices).rstrip(),tail,count=1,flags=re.S)
        tail=re.sub(r'\| Slice name \|.*?(?=\n\n)',lambda _:delivery_table(slices).rstrip(),tail,count=1,flags=re.S)
        path.write_text(head+'## v1.3 delivery slices and acceptance'+tail,encoding='utf-8')

if __name__=='__main__':
    render()
    print('Rendered 27 unique delivery tables; acceptance execution remains NOT_STARTED.')
