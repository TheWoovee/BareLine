"""Validate the documentation package. --refresh-manifest updates the package index."""
from pathlib import Path
import argparse,hashlib,json,re,struct,sys
from urllib.parse import unquote
R=Path(__file__).resolve().parents[1]
parser=argparse.ArgumentParser()
parser.add_argument("--refresh-manifest",action="store_true")
args=parser.parse_args()
errors=[];checks={}
def fail(msg):errors.append(msg)
def files():
 return sorted(p for p in R.rglob("*") if p.is_file() and not any(x in {"graft",".git","__pycache__",".cache"} for x in p.relative_to(R).parts))
def rd(p):return p.read_text(encoding="utf-8-sig")
md=[p for p in files() if p.suffix==".md"]
links=0
for p in md:
 for target in re.findall(r"!?\[[^\]]*\]\(([^)]+)\)",rd(p)):
  target=target.strip("<>").split("#",1)[0]
  if not target or re.match(r"(https?://|mailto:|app:|codex:)",target):continue
  target=unquote(re.sub(r":\d+$","",target))
  q=Path(target) if re.match(r"^[A-Za-z]:/",target) else p.parent/target
  links+=1
  if not q.exists():fail(f"Broken link: {p.relative_to(R)} -> {target}")
checks["local_links_checked"]=links
prs={int(p.name[3:6]):p for p in (R/"PRs").glob("PR-*.md")}
if set(prs)!=set(range(1,28)):fail("Expected 27 PR briefs")
depdoc=rd(R/"04_PR_DEPENDENCIES_AND_PARALLELISM.md")
deps={int(n):set(map(int,re.findall(r"PR-(\d{3})",d))) for n,d in re.findall(r"(?m)^\| PR-(\d{3}) \| ([^|]+) \|",depdoc)}
for n,p in prs.items():
 m=re.search(r"\*\*Depends on:\*\* ([^\n]+)",rd(p))
 own=set(map(int,re.findall(r"PR-(\d{3})",m[1]))) if m else set()
 if own!=deps.get(n):fail(f"PR-{n:03} dependencies differ: brief {own}, table {deps.get(n)}")
seen=set();active=set()
def visit(n):
 if n in active:fail(f"Dependency cycle at PR-{n:03}");return
 if n in seen:return
 active.add(n)
 for d in deps.get(n,[]):visit(d)
 active.remove(n);seen.add(n)
for n in deps:visit(n)
graph={(int(a),int(b)) for a,b in re.findall(r"PR(\d{3}) --> PR(\d{3})",depdoc)}
expected={(d,n) for n,ds in deps.items() for d in ds}
if graph!=expected:fail(f"Mermaid edges differ: missing {expected-graph}; extra {graph-expected}")
checks["dependency_rows"]=len(deps);checks["dependency_edges"]=len(expected)
trace=rd(R/"10_ACCEPTANCE_AND_TRACEABILITY.md")
ids=re.findall(r"(?m)^\| (AC-\d{3}-\d{2}) \|",trace)
if len(ids)!=81 or len(set(ids))!=81:fail("Expected 81 unique acceptance IDs")
for n,p in prs.items():
 for i in range(1,4):
  if f"AC-{n:03}-{i:02}" not in rd(p):fail(f"Missing assigned case in PR-{n:03}")
checks["unique_acceptance_cases"]=len(set(ids))
frs=set(re.findall(r"(?m)^### (FR-\d+(?:A)?) ",rd(R/"01_REQUIREMENTS.md")))
mapped=set(re.findall(r"(?m)^\| (FR-\d+(?:A)?) \|",trace))
if not frs<=mapped:fail(f"Unmapped requirements: {frs-mapped}")
checks["requirements_mapped"]=len(frs)
tracker=re.findall(r"(?m)^\| PR-\d{3} \|.*$",rd(R/"03_PR_TRACKER.md"))
if len(tracker)!=27 or any(row.count("NOT_STARTED")!=5 for row in tracker):fail("Implementation tracker was changed")
checks["tracker_rows_not_started"]=len(tracker)
adrs=set(re.findall(r"(?m)^## ADR-(\d{2}) ",rd(R/"07_DECISION_LOG.md")))
if adrs!={f"{i:02}" for i in range(1,47)}:fail("Expected ADR-01 through ADR-46")
checks["architecture_decisions"]=len(adrs)
def luminance(h):
 vals=[int(h[i:i+2],16)/255 for i in (1,3,5)]
 vals=[v/12.92 if v<=0.04045 else ((v+0.055)/1.055)**2.4 for v in vals]
 return sum(v*w for v,w in zip(vals,[.2126,.7152,.0722]))
def contrast(a,b):
 x,y=sorted([luminance(a),luminance(b)]);return (y+.05)/(x+.05)
tokens=json.loads(rd(R/"mockups/design-tokens.json"));ratios={}
for theme in ["light","dark"]:
 t=tokens[theme]
 for fg in ["text","text.muted","text.gutter","syntax.comment","accent.text","focus.ring","caret","border.interactive"]:
  for bg in ["surface.editor","surface.chrome","surface.elevated"]:
   value=contrast(t[fg],t[bg]);limit=3 if fg in ["focus.ring","caret","border.interactive"] else 4.5
   ratios[f"{theme}:{fg}/{bg}"]=round(value,2)
   if value<limit:fail(f"Contrast below {limit}: {theme} {fg} on {bg}: {value:.2f}")
checks["functional_contrast_pairs"]=len(ratios);checks["minimum_functional_contrast"]=min(ratios.values())
assets=json.loads(rd(R/"mockups/GENERATION_LOG.json"))
for a in assets:
 p=R/a["path"]
 if not p.exists():fail(f"Missing generated asset {a['path']}");continue
 with p.open("rb") as f:
  if f.read(8)!=b"\x89PNG\r\n\x1a\n":fail(f"Invalid PNG {p.name}")
  f.seek(16);dims=struct.unpack(">II",f.read(8))
 if list(dims)!=[a["width"],a["height"]]:fail(f"Dimension mismatch {p.name}")
 for ref in [a.get("supersedes"),a["prompt_file"]]:
  if ref and not (R/ref).exists():fail(f"Missing provenance {ref}")
checks["generated_assets"]=len(assets);checks["superseded_replacements"]=sum(bool(a["supersedes"]) for a in assets)
# Generated report and manifest cannot include a stable self-digest.
excluded={"MANIFEST.json","VALIDATION_V1.2.json"}
if args.refresh_manifest:
 old=json.loads(rd(R/"MANIFEST.json"))
 old.update({"version":"1.2","date":"2026-09-05","architecture_decisions":46,"application_implemented":False,"acceptance_case_count":81,"foundation_contracts":"09_FOUNDATION_CONTRACTS.md","interaction_specification":"11_UI_INTERACTION_SPEC.md","review":"REVIEW_V1.2_2026-09-05.md","generation_log":"mockups/GENERATION_LOG.json"})
 allpaths=[p.relative_to(R).as_posix() for p in files()]
 if "VALIDATION_V1.2.json" not in allpaths:allpaths.append("VALIDATION_V1.2.json")
 old["files"]=sorted(allpaths)
 old["sha256"]={p.relative_to(R).as_posix():hashlib.sha256(p.read_bytes()).hexdigest() for p in files() if p.relative_to(R).as_posix() not in excluded}
 old["hash_exclusions"]={"MANIFEST.json":"Self-referential index","VALIDATION_V1.2.json":"Generated validation output; reruns may change check results"}
 old["asset_status"]={p.relative_to(R).as_posix():("superseded" if "superseded" in p.parts or p.name in {"All_Screens.png","Comparison.png","bareline-hero.png"} else "concept") for p in files() if p.suffix==".png"}
 (R/"MANIFEST.json").write_text(json.dumps(old,indent=2)+"\n",encoding="utf-8")
manifest=json.loads(rd(R/"MANIFEST.json"))
actual={p.relative_to(R).as_posix() for p in files()}|{"VALIDATION_V1.2.json"}
if actual!=set(manifest["files"]):fail(f"Manifest coverage differs: missing {actual-set(manifest['files'])}; stale {set(manifest['files'])-actual}")
for path,digest in manifest.get("sha256",{}).items():
 p=R/path
 if not p.exists() or hashlib.sha256(p.read_bytes()).hexdigest()!=digest:fail(f"Hash mismatch: {path}")
checks["manifest_files"]=len(manifest["files"]);checks["verified_file_hashes"]=len(manifest.get("sha256",{}))
result={"version":"1.2","date":"2026-09-05","passed":not errors,"checks":checks,"errors":errors,"scope":"Documentation/index/asset validation only. No application tests or benchmarks.","contrast_ratios":ratios}
(R/"VALIDATION_V1.2.json").write_text(json.dumps(result,indent=2)+"\n",encoding="utf-8")
print(json.dumps({k:v for k,v in result.items() if k!="contrast_ratios"},indent=2))
sys.exit(bool(errors))

