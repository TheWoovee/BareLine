# Local development

The product authority is docs/blueprint/07_DECISION_LOG.md. Read the active PR brief and its linked contracts before implementation. Track actual delivery in docs/IMPLEMENTATION_STATUS.md; the blueprint copy is a preserved reference.

Work locally. Do not create a remote, push, publish, or deploy until the owner requests it. Implement dependency-ordered slices. Do not mark stubs as completed features.

Start every PR with its own document. Read only its linked contract sections and necessary dependency interfaces; do not reread or analyze the whole blueprint. Keep a concise per-PR implementation/evidence note. Broader reading requires a concrete unresolved question.

Match the current screens in docs/blueprint/mockups exactly as the visual target. Inspect each applicable image before implementing that screen; compare actual rendering at the corresponding dimensions. Preserve composition, spacing, colors, type hierarchy and controls. Track discrepancies explicitly. Use the written interaction contract for behaviors the image cannot express; flag conflicting reference details rather than silently redesigning them.

Keep verification proportional: focused behavioral tests for consequential invariants, one integration compile after wiring, no full build/test loop for cosmetic or documentation fixes. Reuse Cargo's incremental outputs. Do not add placeholder crates or tests that merely restate implementation.

Core source headers: MPL-2.0. Protocol/SDK source headers: MIT OR Apache-2.0. Keep Windows types inside platform-windows and renderer-d2d. No async or browser runtime.

The owner confirmed the PC is unlocked and authorized native testing on 2026-09-06. Use isolated generated scratch data for native verification; do not change the lock state or interfere with personal apps. If the desktop becomes locked, stop desktop automation and retain visual/input/IME checks as pending until it is unlocked again.

Track every lock-blocked check in docs/UNLOCK_CHECKLIST.md with an actionable resume step. When unlocked, follow its order and record actual evidence before closing items. Keep ordinary implementation gaps separate from lock-only blockers.
