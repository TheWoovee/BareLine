# SPDX-License-Identifier: MPL-2.0
"""Concrete acceptance fixtures/actions awaiting candidate execution and review."""
import hashlib
import json
from pathlib import Path

ROOT=Path(__file__).resolve().parents[2]

# Three deliberately distinct atomic fixtures per authority group. These are
# procedural inputs, never observations, evidence IDs or reviewer attestations.
CASES={
1:[
 "Use exact text 'abc العربية é 👩🏽‍💻 xyz'. Capture real DirectWrite glyph/caret geometry, commit/cancel installed IME composition at 100/150/200% DPI, then trigger owned device recreation and compare text/caret plus first recovered frame.",
 "Generate a session with 5,000 unique lazy paths under owned scratch and instrument document reads/startup events. Launch once; compare timestamps proving first frame precedes every document read and record peak worker count.",
 "Compile codec, LocalFileSystem and VerifiedPackageSource consumers against fake denied/partial providers. Attempt online-provider access without a configured implementation; require explicit unsupported/denied and zero fallback access.",
],
2:[
 "Use deterministic seed 123456789abcdef and 100,000 insert/delete/replace operations over CR, LF, CRLF, ASCII, Arabic and emoji. Keep a separate flat UTF-8 reference and compare bytes/line starts after each operation and complete Undo/Redo.",
 "Create three base pages with distinct byte patterns, delete a range crossing both page boundaries, force base-page eviction, then replace the source with different same-length bytes. Undo must use retained inverse bytes and match the original sequence exactly.",
 "Hold a resident background read after it captures its generation; truncate then rewrite the source with a distinguishable pattern before releasing it. Every missing old-generation interval returns Unavailable; no new pattern may appear in the old snapshot.",
],
3:[
 "Use 'a👩🏽‍💻éb' plus an invalid-byte provenance fixture containing FF and genuine U+FFFD. Exercise forward/backward and rectangular selection, delete, paste and Undo; record raw/text offsets and reject boundaries inside clusters/opaque spans.",
 "Stream-generate one 20 MiB ASCII line with a final marker. Scroll to middle/end and reverse quickly while layout is pending; retain memory/shape-work counters, cancellation and real input response rather than waiting for a full-line layout.",
 "On real Windows with installed Japanese/Chinese and an AltGr layout, commit one composition then cancel another inside mixed Arabic/Latin text. Compare document, Undo and recovery to only the committed text; capture candidate-window geometry and BiDi hit testing.",
],
4:[
 "Open clean 'A', insert B and save AB. Insert C, then Undo once: content AB and clean saved ContentState must return even though revision increased. Redo ABC must become dirty; record save identity, not a text comparison.",
 "Use owned saved and Untitled documents with numbered acknowledged edits. At each declared segment/journal/checkpoint flush boundary pause the owned writer, terminate it and reopen once. Compare recovered text/provenance with the last fully durable prefix; corrupt final records are excluded.",
 "Pause Save after capturing AB, type C, then allow the AB save to finish. Disk AB is valid while buffer ABC remains dirty. In a second run invalidate uncached source pages: Save/Save As are refused and any gap export is explicitly labelled incomplete.",
],
5:[
 "Generate a 20 MiB single line with a mixed-case Unicode needle straddling the 16 MiB scan boundary. Compare full-match offsets with an independent byte search/case-fold mapping and require exact original ranges, including length-changing folds.",
 "Use multiline 'aa\\nbb' with ^, $, lookbehind, empty matches and a capture spanning an unavailable window. Compare supported complete matches; incomplete/unsupported context must be labelled and cannot authorize replacement.",
 "Open two dirty buffers containing one each, prepare replacement one->X, then change one revision before group commit. Neither may change. Reset, complete the linked transaction and Undo from either participant; both original buffers restore.",
],
6:[
 "Use tests/e2e/column_fixture.py exact tabs/CJK/short-row fixture and expected INSERTED bytes. Insert the declared sequence by display column, save, Undo once and compare exact INITIAL bytes and selection geometry.",
 "Enable Clipboard History in a clean profile; copy 21 distinct entries, then entries totaling more than 16 MiB. Inspect bounded eviction, disable/re-enable and restart. Neither profile files nor restarted history may contain copied text.",
 "Use an explicit provider-absent language fixture through the normal CommentProvider hook, not a currently supported Rust language. Invoke line/block comments and require unavailable reason plus unchanged bytes/revision/history. A stub-provider control verifies dispatch wiring.",
],
7:[
 "Open exact raw byte fixtures for invalid UTF-8 (41 FF 42), genuine EF BF BD, private-use EE 80 80 and noncanonical legacy sequences. Edit around preserved spans, Undo and save; compare original-byte tags and exact byte roundtrip without marker collisions.",
 "Generate Aé plus mixed CRLF/LF/CR in UTF-16LE/BE and UTF-32LE/BE with appropriate BOM, plus Latin-1 bytes 41 80 FF. Open/save untouched and compare every raw byte; Latin-1 80 must remain U+0080 rather than Windows-1252 euro.",
 "Stream-generate exactly 5 GiB UTF-16 with unique chunk-boundary markers. Pin configured RAM/disk limits; open viewport-first and transcode. Exhaust the owned temp quota and verify visible pause/failure with original bytes intact; resume only with an explicitly increased quota.",
],
8:[
 "Use all 15 catalog languages, each with code, string, comment, nested delimiter and Unicode fixtures. Run both native and Lexilla providers; compare token/fold snapshots to declared expected grammar coverage and retain intentional unsupported grammar cases individually.",
 "Generate a multiline string/comment spanning several lexer windows and open initially in its middle. Retain provisional label, checkpoint source/revision and final spans; editing the opener invalidates only dependent bounded work and stale spans cannot appear current.",
 "Start with the valid qaudl definition and styled sample in tests/e2e/udl_fixture.py. Import malformed XML/JSON, DTD/external entity, invalid IDs and each size-limit-plus-one input; old in-memory and durable definitions remain identical across restart.",
],
9:[
 "Open root/main.rs='fn main() {}' then Cargo.toml='[package]'. Delay the Rust outline reply until after selecting TOML; no Rust symbol may appear/click into TOML. Switch back and require current Rust symbols with exact offsets.",
 "Create an owned directory graph containing a link back to an ancestor and an inaccessible child. Expand repeatedly; enforce entry/depth/work caps, no loop, explicit partial/error rows and no traversal outside the authorized root.",
 "Generate a 5 GiB numbered log and scroll before indexing completes. Record estimated counts/positions and minimap/outline allocation counters; there must be no per-line full-file allocation or fabricated exact line total.",
],
10:[
 "Open three distinct tabs, pin the middle one, move/reorder across two panes, then save/restore the session. Compare pinned-first grouping, active document per pane, stable tab/document identities and independent caret state.",
 "Compare left='a\\nb\\n' to right='a\\nx\\ny\\nb\\n'. Capture alignment spacer rows; only actual document rows have line numbers. Scroll both directions and verify logical mappings rather than equating display-row numbers.",
 "Clone one 200-line document into two differently sized panes; set distinct carets, selections, folds and scroll. Edit one pane and Undo; shared text updates in both while each pane's independent view state remains.",
],
11:[
 "Export the exact composed runtime command inventory with trusted installed contributions. Execute each catalogue success/disabled/failure procedure through its actual menu/toolbar/palette/control route, preserving IDs and reasons; no handler/count-only substitute qualifies it.",
 "Open a nested popup over a panel over the editor, including an active IME preedit. Press Escape one layer at a time and record visibility/focus/text; only the topmost transient layer closes or cancels at each press.",
 "Use an installed AltGr layout and explicit remapped shortcut colliding with a possible Ctrl+Alt interpretation. Type the intended character, use a menu mnemonic and invoke the shortcut separately; text must not be consumed as a global command.",
],
12:[
 "Set editor font to exactly 12 pt, move between actual 100/150/200% DPI monitors, preview and restart. Compare persisted scalar 12 and device-dependent rendered size; no conversion back into stored pixels.",
 "Create owned workspace settings setting tab width=4 and attempting execution, network and extension-grant overrides. Load it; indentation changes while privileged keys are rejected with reasons and user grants remain unchanged.",
 "Capture real Light/Dark/System editor, modal, disabled, selection, error and focused surfaces with custom overrides. Measure normal-text/background ratio >=4.5:1 and focus >=3:1; change Windows theme and restart to verify override persistence.",
],
13:[
 "Use Rust/Python and a test registered CommentProvider with exact line/block tokens; apply toggles at an empty caret and selected Unicode text. Second toggle and Undo must restore exact bytes without duplicating or removing unrelated delimiters.",
 "Request completion for prefix alp, pause the worker, type another character or switch document/language, then deliver its old reply. It must not replace newer text; a fresh current suggestion replaces only its declared prefix.",
 "Use identifier alp in code, a quoted string, line comment and block comment, with an unrelated open document containing similar names. Inspect bounded index size and context classification; unsupported semantic contexts suppress misleading suggestions.",
],
14:[
 "Use tests/e2e/macro_fixture.py, record literal text plus a search with explicit arguments, restart and replay. Inspect versioned events and exact FINAL text; insert one failing command and verify the resumable event index does not repeat prior acknowledged edits.",
 "Run external_fixture.py through a direct command definition containing macro_fixture.ARGS, an executable/path with spaces and an empty argument. Compare the child's observed argv/cwd byte-for-byte; shell metacharacters must remain literal.",
 "Run the owned external fixture in unlimited-output mode with one owned child. Cancel; retain output byte/entry caps and actual process creation/exit identities. Both owned processes terminate, unrelated processes remain and editor accepts a subsequent edit.",
],
15:[
 "Create a log whose final partial page ends before a known marker; append enough bytes to complete that page and create the next one. Compare the full continuity hash and every boundary byte; accepted generation never loses/duplicates the old suffix.",
 "In separate resets rotate the log, truncate it, and replace it with same-size different bytes while retaining an owned edit. Require SourceChanged/explicit review when continuity is uncertain, with old owned/inverse bytes preserved.",
 "Follow a growing log, pause scrolling and append three numbered chunks. Viewport stays fixed while new data remains available. Confirm Unlock: fixed generation becomes editable and later appends are not silently adopted; cancel keeps following.",
],
16:[
 "Provide reviewed signed offline runtime, catalogue and JSON/XML/Hex packages matching a configured diagnostic candidate. Install through the provider and editor manager paths; capture exact signed bytes, publisher/root floors and explicit capabilities with network offline.",
 "Start an owned extension job, disable extensions and record host termination. Installed packages remain. Explicitly Remove runtime; only its verified owned files disappear, with unrelated package/profile/document hashes unchanged.",
 "Use three reviewed hostile components: oversize protocol frame, ungranted capability request and CPU loop. Invoke through the real host; enforce declared frame/grant/fuel/time/process limits, discard partial edits and verify editor survival.",
],
17:[
 "Compare left='left\\nkeep\\n' and right='right\\nkeep\\n'; copy first hunk left-to-right. Only right becomes dirty with left's first line; labels/target selection match identities. One Undo restores right and no disk file was auto-saved.",
 "Use separate case, blank, whitespace, EOL and tab-only differences. Toggle each option and recompare current revisions; cancel during work, then force a coarse result. Distinguish Cancelled/CompletedCoarse and disable stale merge actions after an edit.",
 "Export/print '<script>& café 🙂' with syntax/theme options and a selected second line. Validate HTML escaping, RTF Unicode and print range/header/footer/units. Denied destination and unavailable printer preserve originals and expose retry.",
],
18:[
 "Use a configured candidate and six signed metadata vectors: valid, expired, lower version, wrong channel, wrong publisher and wrong artifact hash. Inspect stage/install directories before/after; every rejected vector changes no application file.",
 "Run two isolated unsigned configured builds with identical inputs; compare required payload hashes. Then verify final signed ZIP/installer extraction and every inner PE against actual signer policy; signing-byte differences cannot serve as reproducibility proof.",
 "Create paths containing 文 and 🎉 and arguments including spaces/quotes; use authenticated same-user IPC between owned instances. Repeat with spoofed peer and portable relocation; only the selected portable data root receives profile writes.",
],
19:[
 "Pin two executable hashes and identical generated workloads/hardware/environment. Alternate AB/BA trial order, separating cold/warm states and first frame/editable/full load. Keep timeouts and failed trials in raw data rather than dropping them.",
 "Measure 100 and 500 lazy/loaded tabs with an active owned extension host. Sample full process-tree peak private bytes and startup/download/disk counters; child memory is included and incomparable workload states stay separate.",
 "Retain all paired trial logs and exact aggregation inputs, recompute sample counts/P50/P95/timeouts, and publish both distributions with environment limits. Hosted-CI timing never becomes an unsupported product comparison.",
],
20:[
 "Run each enumerated recovery/save/replace-receipt transition with disk-full and owned process-death injection. Pin the observed boundary and reconcile target, backup, journal and checkpoint fingerprints; never infer a boundary from elapsed sleep.",
 "Use traversal archives, owned junction escapes, spoofed IPC peers, expired/rollback metadata and real signed root-rotation/recovery vectors. Reject before unsafe writes or invocation; accepted recovery requires separately authenticated offline-root policy.",
 "Place unique document-text and sensitive-path canaries in generated inputs. Exercise errors, diagnostics and recovery; scan all diagnostic artifacts for canaries and monitor actual network activity. No content/path disclosure or telemetry upload is allowed.",
],
21:[
 "Resolve the final composed command inventory plus all atomic/environment cells against retained source/artifact-bound procedures. Every imported record needs independent review; missing IDs, duplicate conflicts, failed cells and unsupported exclusions keep readiness incomplete.",
 "Run all 13 journey procedures with keyboard and real Narrator on both declared Windows floor builds and mixed DPI. Retain actual native artifacts/host facts for each cell; headless procedures and mock surfaces cannot fill native cells.",
 "Produce the prerequisite readiness report from exact final inputs, obtain independent assessment, and bind its immutable receipt in final closure. List unresolved correctness, measured performance and limited parity claims; no circular self-certification.",
],
22:[
 "On actual Linux/macOS CI hosts compile neutral crates with denied Windows imports and exercise unsupported filesystem/renderer capabilities. Windows mocks are controls only; real host receipts remain owner-deferred to next update.",
 "Roundtrip versioned paths containing unpaired Windows UTF-16 units and Unix raw non-UTF-8 bytes through serialization/session state. Compare raw units/bytes, not lossy display strings; reject invalid cross-platform operations explicitly.",
 "Run the same RecordingBackend contract tests on each actual OS and retain separate host/toolchain receipts and readiness rows. Do not label them complete platform ports; foreign-host execution remains deferred.",
],
23:[
 "Enumerate controls in every current modal/popup/panel from the accessibility fixtures. Traverse Tab/Shift+Tab/arrows from each boundary; record visible focus and disabled command rejection with no accidental edits.",
 "Use editable fields for Find, Settings, Run and color values with IME preedit, Unicode clipboard and invalid values. Cancel/dismiss; preedit must not enter committed text/Undo and focus returns to the owning surface.",
 "At actual 100/125/150/200/250% DPI capture paint and hit rectangles for all controls, including popup near each screen edge. Hit centers/boundaries and verify correct target plus on-screen clamping without stale scale.",
],
24:[
 "Use real Narrator through generated open/edit/find, an external-save conflict and eligible recovery. Record spoken names/roles/values/focus and keyboard route; semantic-tree snapshots alone do not establish the speech result.",
 "Open a generated 5 GiB log, select/scroll across a page boundary and issue UIA TextRange requests near viewport and far offsets. Record bounded bytes/pages/work and cancellation; no full document materialization or fake empty ranges.",
 "Enable actual Windows high contrast and inspect focus, selection, disabled and error states without relying on color alone. Use real DirectWrite shaping plus source-identical headless controls; retain the actual high-contrast environment separately.",
],
25:[
 "Use deterministic Unicode/CRLF/case/whitespace fixtures and random seed 123456789abcdef. With ignores off, apply all hunks and compare exact target bytes; with each ignore on, compare only explicit nonignored ranges and retained target formatting.",
 "Use the existing forced normalized-hash-collision seam with distinct normalized rows and case/space-equal controls. Compare normalized bytes after hash equality; returned edit ranges must remain original byte ranges through length-changing case folds.",
 "Generate divergent multi-GiB sources under explicit anchor/output/RAM caps; trigger completed coarse, explicit cancel and unavailable source independently. Verify distinct terminal enums/labels and prohibit exactness claims for incomplete output.",
],
26:[
 "Preview one->X in two disk files, then replace one externally before Apply. Only the unchanged reviewed target is eligible; changed file is skipped for re-review without re-matching unseen text or overwriting it.",
 "Mix two dirty open documents with two disk-only targets, one alias path and one denied destination. Capture current revisions/fingerprints; apply reviewed rows with unique backups and durable per-file receipts, and reconcile partial success honestly.",
 "Pause after actual target replacement but before receipt commit, terminate the owned process, then restart reconciliation once. Compare target/backup fingerprints to original/replacement; never blindly replay or overwrite a subsequently changed target.",
],
27:[
 "Format/minify JSON with 9007199254740993, 1e+30, -0 and a deeply nested array within declared limits. Compare number lexemes/value policy exactly and verify one Undo restores source bytes; over-limit/malformed input fails without partial edits.",
 "Use valid XML plus DTD, external entity, XInclude, unsupported XPath, excessive depth and CPU-heavy fixtures. Real host denies network and enforces budgets; each unsafe/unsupported case is explicit failure, not empty success.",
 "Open raw bytes FF FE 41 00 in original-byte Hex, make an unsaved text edit to B, then request encoded edited preview separately. Original mode stays FF FE 41 00 at the original generation; preview is explicitly distinct and never substituted silently.",
]}


def bindings():
    backlog=json.loads((ROOT/'docs/implementation/production-readiness/BACKLOG.json').read_text(encoding='utf-8-sig'))
    owners={row['id']:row for row in backlog['items'] if row['id'].startswith('QUAL-')}
    result={}
    for group,cases in CASES.items():
        owner=owners[f'QUAL-{group:03}']
        authority=[dict(path=path,sha256=hashlib.sha256((ROOT/path).read_bytes()).hexdigest()) for path in owner['sources']]
        for number,recipe in enumerate(cases,1):
            key=f'AC-{group:03}-{number:02}'
            result[key]=dict(procedure_id='procedure:'+key,owner=owner['owner_role'],fixture_and_actions=recipe,
                observations=['Exact input recipe/hash and candidate/source identity','All clause outcomes, including cancellation/rejection paths',
                              'Before/after bytes, offsets/state and relevant bounded resource observations','Terminal result, owned cleanup and retained raw artifacts'],
                authority=authority,status='NOT_RUN',review='pending',complete_case_mapping=False,
                required_execution='Real host/physical/signed/lab conditions in this recipe cannot be replaced with mocks',
                owner_deferred=group==22)
    if len(result)!=81:
        raise ValueError('Atomic procedure count changed; review authority')
    return result
