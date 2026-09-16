# DEV-002 — Native focus and Unicode input

**State:** implemented and focused-verified, 2026-09-15; broader qualification remains pending. **Owner:** Windows input/accessibility.

## Scope and authority

Follow [DEV-002 in the backlog](BACKLOG.md), the [final native qualification limits](../2026-09-12-audit-implementation/FINAL_ACCEPTANCE.md), and the focused input/evidence contracts in `xtask/src/journey.rs`. Establish actual Editor provider focus and exact Unicode delivery before native assertions; distinguish provider, product and driver faults. Preserve one Close/Exit and owned scratch data. Do not reopen consent behavior without a reproduction.

## Findings and implementation

- **Astral packet input:** the original p4-4 failure reproduced with complete SendInput submission but empty text. Winit 0.30.13 finalizes each VK_PACKET key's UTF-16 text separately, dropping unpaired surrogate units. The old app surrogate helper was test-only. A bounded decoder now runs in the real Windows message hook, holds the high unit, and synchronously supplies the completed scalar through winit's existing WM_CHAR scalar handling and normal keyboard/field route. The unused helper/test was replaced by regressions on the live decoder.
- **Input ownership:** only the explicitly registered app HWND is intercepted. Native dialogs retain their normal path. Native focus changes, accessibility actions and modal activation advance an input epoch; partial scalars cannot cross that boundary. Paint/timer/worker messages preserve a pending pair. A first native iteration found an overly broad reset dropping one emoji; that failed result remains retained.
- **UIA focus:** p0-3 initially passed, then the missing Editor-focus state reproduced. The accessibility adapter now reconciles the current owning thread's real HWND focus before snapshot equality can skip an update. Native menus, move/size loops and other focused windows are excluded. A small wrapper in the already-vendored AccessKit adapter exposes its existing focus-state update outside WM_GETOBJECT. This resolves the observed stale host-focus state without assigning focus merely to satisfy a test.
- **Settings:** after the common fix, six input surfaces passed but Settings remained empty. Its text handler only accepted a logical Character key; VK_PACKET has an Unidentified logical key with valid event.text. Settings now accepts that decoded text with the existing modifier/composition guards.

## Focused verification

[Retained verification](../../qa/2026-09-15-dev002-native-input/README.md) contains exact commands, source identities, failed attempts and final results.

- **10 unit tests passed:** 9 packet/boundary regressions and 1 native-focus ownership/menu-state regression. No full platform or app suite ran.
- **13 native probe checks passed:** exact `A🎉é文` in Editor, Find, Replace with, Go To, Run, command palette, Settings and both split panes; distinct UTF-16 carets 0/6; F6 from Pane 2 to Pane 1 and back; split provider retirement and Editor focus restoration. The probe uses returned numeric runtime IDs, focus and TextPattern selections. It does not infer ownership from shared buffer text alone.
- **p0-3, p0-7 and p4-4 passed** on one final binary. These preserve the existing exact-text oracle, single Exit/Close submission, consent and p0-3 restart/discard checks; emoji rendering is evaluated only after exact input delivery.
- Final debug binary SHA-256: `b52da9546ac1547a75a9679c8afa4d0c70c23fd4c23b57219112a58b54aff414`. Incremental debug builds were used while refining the patch; no release build or full workspace suite ran. The final build receipt records its exact source identity. A subsequent formatting-only change affected the new cfg(test) block.
- Local diff review checked HWND ownership, message/epoch resets, synchronous dispatch without modifying the borrowed MSG, native-dialog pass-through, focus update timing and existing Settings guards. No independent reviewer was used.

## Remaining qualification and stopping point

The native Close/Exit checks did not assert that recovery I/O was busy at the exact instant of command submission. Deterministic busy/resumption stress remains an explicit final qualification gate; these passes do not establish it. Full suites, release builds, physical IME/screen-reader/platform coverage, the complete historical U08 interaction sequence and independent review remain deferred. No capability family or atomic acceptance case was promoted by this slice.

Stopped after DEV-002 implementation and focused verification as requested. DEV-003 and later items were not started.
