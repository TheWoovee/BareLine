# Local development

The product authority is docs/blueprint/07_DECISION_LOG.md. Read the active PR brief and its linked contracts before implementation. Track actual delivery in docs/IMPLEMENTATION_STATUS.md; the blueprint copy is a preserved reference.

Work locally. Do not create a remote, push, publish, or deploy until the owner requests it. Implement dependency-ordered slices. Do not mark stubs as completed features.

Start every PR with its own document. Read only its linked contract sections and necessary dependency interfaces; do not reread or analyze the whole blueprint. Keep a concise per-PR implementation/evidence note. Broader reading requires a concrete unresolved question.

Match the current screens in docs/blueprint/mockups exactly as the visual target. Inspect each applicable image before implementing that screen; compare actual rendering at the corresponding dimensions. Preserve composition, spacing, colors, type hierarchy and controls. Track discrepancies explicitly. Use the written interaction contract for behaviors the image cannot express; flag conflicting reference details rather than silently redesigning them.

Keep verification proportional: focused behavioral tests for consequential invariants, one integration compile after wiring, no full build/test loop for cosmetic or documentation fixes. Reuse Cargo's incremental outputs. Do not add placeholder crates or tests that merely restate implementation.

Core source headers: MPL-2.0. Protocol/SDK source headers: MIT OR Apache-2.0. Keep Windows types inside platform-windows and renderer-d2d. No async or browser runtime.

The owner confirmed the PC is unlocked and authorized native testing on 2026-09-06. Use isolated generated scratch data for native verification; do not change the lock state or interfere with personal apps. If the desktop becomes locked, stop desktop automation and retain visual/input/IME checks as pending until it is unlocked again.

Track every lock-blocked check in docs/UNLOCK_CHECKLIST.md with an actionable resume step. When unlocked, follow its order and record actual evidence before closing items. Keep ordinary implementation gaps separate from lock-only blockers.

<!-- graft:start -->
## Graft — repo context graph

This repo is indexed in `graft/`: small linked markdown nodes that explain each
system and carry exact file:line spans, kept in sync with the code through git.

For ANY task here — understanding how something works, finding where code lives,
or scoping a change — get context from the graph before grepping or opening
source files. Re-ask freely (it's cheap) and reuse literal identifiers you
already have (symbol, error string, file name) as the query. New to this repo?
Run `graft map` first — a token-budgeted orientation (dir clusters, hubs,
hotspots), no LLM, no key.

- Run `graft ask "<your question>" --source` → ranked nodes with the relevant
  code spans inlined (each hit's ≤8-line crux by default; `--full` for whole
  definitions when the crux isn't enough). Match the tool to the task shape:
  for understanding or editing, the top node IS the answer — cite its
  `covers:` file:line spans and edit straight from `--source`. For
  exhaustive tasks ("every occurrence / every caller of this pattern"), ranked
  results are top-N, not complete — run `graft grep "<literal>"` instead
  (exhaustive over indexed files, grouped by enclosing symbol), falling back
  to raw `grep -rn` only for unindexed files.
- `graft skeleton <file>` → every definition's signature + span, ~10× cheaper
  than reading the file; use it to skim an API surface.
- `graft callers <symbol>` gives precomputed, exact edges — who calls this.
  Add `--direction out` for what it calls, or `--depth N` to walk
  transitively for the full blast radius. For structural questions, skip
  ranking and use this directly.
- Or browse: `graft/INDEX.md` lists every node; follow the links.
- Monorepos and folders of multiple repos rank fairly across sub-projects —
  hits carry `[scope/]` labels naming which one they're from. Narrow with
  `graft ask "<task>" --in <scope>/` once you know where you're working.

If a returned span is truncated ("+N more lines"), open the file at that exact
range before finalizing. Only open source files when a node genuinely lacks a
needed detail, and then at the exact file:line the node points to — never
re-read whole files.

After big code changes, refresh the graph with `graft build` (deterministic,
no API key, $0).
<!-- graft:end -->
