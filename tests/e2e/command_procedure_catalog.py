# SPDX-License-Identifier: MPL-2.0
"""Source-backed manual procedures. Authoring is separate from observed acceptance.

This catalogue deliberately enumerates supported IDs. New runtime contributions
remain unbound until their actual contracts are added; no name-based fallback is
allowed to make a command appear covered.
"""
import hashlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
APP = 'apps/bareline/src/windows_app/'
CORE = 'crates/app/src/'
PURE = ('Proposed inapplicable: this operation only changes the stated view/control state; '
        'the cited handler has no independent fallible operation. Verify the state transition and '
        'review this rationale separately; do not import it as an exclusion or a PASS.')
NO_DOCUMENT = 'Close all documents or invalidate the selected document identity; the operation must not mutate another document.'
READ_ONLY = 'Set the destination read-only, then repeat with an edit pending; reject without bytes, revision, disk or Undo changes.'
STALE_EDIT = 'Capture the operation, edit its destination before completion, then deliver the old result; reject the stale result and preserve the newer bytes and Undo history.'
UI_ONLY = 'No file bytes, dirty state or Undo history change. Capture the control state and resulting focus.'


def catalog():
    rows = {}

    def add(ids, setup, action, expected, unavailable, failure, sources, mutation=False):
        if isinstance(ids, str):
            ids = ids.split()
        for command in ids:
            if command in rows:
                raise ValueError('Duplicate concrete command: ' + command)
            rows[command] = dict(command=command, setup=setup, action=action,
                                 expected=expected, unavailable=unavailable, failure=failure,
                                 sources=sources, mutation=mutation)

    compare = [APP+'compare.rs', CORE+'compare.rs', 'crates/diff/src/lib.rs']
    pair = "Open left.txt='left\\nkeep\\n' and right.txt='right\\nkeep\\n', activate left, open Compare and wait for a current completed result with ignore options off."
    for direction, source, target in [('LeftToRight','left','right'),('RightToLeft','right','left')]:
        add('compare.copy'+direction, pair, 'Select the first differing hunk and invoke the indicated direction once.',
            f"Only {target}.txt becomes '{source}\\nkeep\\n'; it becomes dirty, the source stays unchanged, and one Undo restores the destination's original bytes. No implicit save.",
            READ_ONLY+' Also invalidate either compare revision; stale hunk actions cannot commit.', STALE_EDIT, compare, True)
        add('compare.copySelection'+direction, pair+' Select bytes [0,4) in left and [0,5) in right.',
            'Invoke the selected-range copy in the indicated direction.',
            f"The explicit source selection replaces only the {target} selection; the trailing '\\nkeep\\n' remains identical. One Undo restores the exact original destination and selection.",
            READ_ONLY+' Empty source selection reports Select source text to copy first.', STALE_EDIT, compare, True)
    for suffix, option, fixture in [
        ('ignoreCase','ignore_case',"left='Alpha\\n', right='alpha\\n'"),
        ('ignoreBlank','ignore_blank_lines',"left='a\\n\\nb\\n', right='a\\nb\\n'"),
        ('ignoreEol','ignore_eol_style',"left bytes=61 0d 0a, right bytes=61 0a"),
        ('ignoreBom','ignore_encoding_bom',"identical UTF-8 text; only left has EF BB BF"),
        ('normalizeTabs','normalize_tabs',"left='\\tx\\n', right='    x\\n', tab width=4"),
    ]:
        add('compare.'+suffix, pair+' Reset '+option+' to false; use '+fixture+'.',
            'Toggle once, wait for recompare, then toggle off and recompare again.',
            option+' is true then false; the named difference is ignored only while enabled. Capture completeness and source revisions; both documents remain unchanged.',
            'Close Compare; this compare-context action cannot create a comparison or edit.',
            'Cancel the recompare and then change either source; cancelled/stale results must not appear as current completed output.', compare)
    for suffix, states in [('whitespace','Significant -> TrimEdges -> IgnoreAll -> Significant'),
                           ('trimEdges','Significant -> TrimEdges -> Significant'),
                           ('ignoreWhitespace','Significant -> IgnoreAll -> Significant')]:
        add('compare.'+suffix, pair+" Add edge-space and internal-space-only differences; set Significant.",
            'Invoke repeatedly through the declared cycle, waiting for each recompare.',
            states+'; edge-only and internal whitespace differences follow the selected policy; document bytes never change.',
            'No active compare pair; no comparison or mutation is created.',
            'Interrupt recompare; cancelled output remains distinct from CompletedCoarse and merge stays unavailable.', compare)
    for suffix, field in [('pauseAutomatic','pause_automatic'),('syncHorizontal','sync_horizontal')]:
        add('compare.'+suffix, pair+' Record '+field+'=false.', 'Toggle on, exercise it, then toggle off.',
            field+' changes false -> true -> false. For pause, editing leaves results stale until explicit recompare; for sync, horizontal scroll follows only while enabled.',
            'Close the compare pair; no unrelated pane or document changes.', PURE, compare)
    for suffix, expected in {
        'open':'Two ordinary editor panes compare the active document with the first other open document; source labels and hunk ranges match their actual identities.',
        'close':'Compare options and aligned comparison close; original documents and all unsaved edits remain open.',
        'options':'Compare Options opens on Colors with first control focused; invoking again closes that modal.',
        'generalTab':'Only the General tab becomes active; colors and source bytes remain unchanged.',
        'colorsTab':'Only the Colors tab becomes active; compare options and source bytes remain unchanged.',
        'swap':'Left/right source identities and labels swap and a new comparison starts; file contents do not swap.',
        'leftSource':'The left source advances to the next open document that is not the right source.',
        'rightSource':'The right source advances to the next open document that is not the left source.',
        'next':'Current difference advances to the next hunk and both panes navigate to its logical source offsets.',
        'previous':'Current difference moves to the previous hunk and both panes navigate to its logical source offsets.',
        'recompare':'A new comparison is requested for the current two revisions; only those revisions may publish mergeable hunks.',
        'cancel':'Comparison and pending merge preparation stop; no partial merge is inserted and cancellation is distinguishable from a completed coarse result.',
        'disk':'The chosen owned disk file is opened as the other compare source with its actual Disk origin and path.',
        'external':'The active document is compared against its current external disk version; unsaved in-memory edits are retained.',
        'lastSaved':'The other source is the captured last-saved snapshot, labelled last saved; later in-memory or external disk edits do not change that snapshot.',
    }.items():
        setup = pair+" Also open third.txt='third\\nkeep\\n'; for next/previous use two separated differing hunks; for lastSaved save 'old' then edit to 'new'; for external rewrite disk to 'disk' while memory remains 'new'."
        unavailable = 'Use one document for open; no current pair for pair actions, no saved snapshot for lastSaved, and no associated path for external. No unrelated document may change.'
        failure = 'Cancel the owned file picker or make its selected source unavailable; report the failure and preserve the current documents.' if suffix in {'disk','external','lastSaved'} else PURE
        add('compare.'+suffix,setup,'Use the matching Compare toolbar/options action; internal panel actions are invoked from their panel, not fabricated palette entries.',expected,unavailable,failure,compare)
    for state in ['Added','Removed','Changed','Moved','Current']:
        for control,key_suffix in [('color',''),('gutter','.gutter'),('accent','.overview')]:
            key='diff.'+state.lower()+key_suffix
            add('compare.'+control+state,pair+' Open Compare Options > Colors; record '+key+'.',
                'Activate this color control.', 'Color input owns '+key+'; it shows the current #RRGGBB value selected for editing. No setting is changed until Apply.',
                'Close the options modal; no hidden field may receive typing or change settings.', PURE, compare)
        add('compare.reset'+state,pair+' Set '+state.lower()+" diff overrides plus an unrelated editor override.",
            'Reset this semantic color row.', 'Remove only diff.'+state.lower()+' and its dotted child overrides; the unrelated override remains. Effective color returns to the current theme.',
            'Read-only settings store must not silently claim persisted changes.',
            'Inject an owned settings-write denial; expose persistence error and retain the previous durable settings.',compare)
    for theme in ['Light','Dark','System']:
        add('compare.theme'+theme,pair+' Open Compare Options > Colors.', 'Choose the indicated theme.',
            'theme.mode becomes '+theme.lower()+' and survives restart; System follows the actual Windows theme. Existing custom overrides remain.',
            'Settings unavailable/read-only: retain honest pending/error state.',
            'Deny the isolated settings write; show failure and preserve the prior durable file.',compare)
    add('compare.applyColor',pair+" Select Added background, type '#123456'.",'Apply the color, then repeat with invalid #GGGGGG.',
        'Valid input changes only diff.added; invalid input keeps the previous value and shows field validation. Source bytes remain unchanged.',
        'No active color field: no unrelated theme key is changed.', 'Deny the isolated settings write; report error without corrupting the file.',compare)
    add('compare.defaults',pair+' Set all five semantic diff overrides and one unrelated editor override.', 'Restore compare defaults.',
        'All diff.* overrides are removed, color_blind becomes false and the unrelated override remains.',
        'Unavailable settings store cannot silently claim persistence.', 'Deny settings write and preserve previous file.',compare)
    add('compare.colorblind',pair+' Reset diff overrides; color-blind preset starts off.', 'Toggle preset on then off.',
        'On: added=#DCEAF7, removed=#FAE5CF, added.gutter=#2474B5, removed.gutter=#B96312. Off removes these four overrides; unrelated keys remain.',
        'Unavailable settings store cannot silently claim persistence.', 'Deny settings write and expose failure.',compare)

    encoding=[APP+'encoding.rs',CORE+'encoding.rs',CORE+'workspace/encoding.rs']
    codecs={'utf8':('utf-8','Aé🙂'),'utf16le':('utf-16le','Aé🙂'),'utf16be':('utf-16be','Aé🙂'),
            'utf32le':('utf-32le','Aé🙂'),'utf32be':('utf-32be','Aé🙂'),'latin1':('latin-1','Aéÿ'),
            'windows1250':('cp1250','Ač'),'windows1251':('cp1251','AЖ'),'windows1252':('cp1252','A€'),
            'windows1253':('cp1253','AΩ'),'windows1254':('cp1254','Ağ'),'windows1255':('cp1255','Aא'),
            'windows1256':('cp1256','Aش'),'windows1257':('cp1257','Aā'),'windows1258':('cp1258','Ađ'),
            'shiftjis':('shift_jis','Aあ'),'gbk':('gbk','A中'),'big5':('big5','A中'),
            'eucjp':('euc_jp','Aあ'),'euckr':('euc_kr','A한')}
    for suffix,(codec,text) in codecs.items():
        raw=(text+'\r\nx\n').encode(codec).hex()
        add('encoding.interpret.'+suffix,'Create owned bytes '+raw+'; record the original hash and open the file. Repeat with one unsaved edit.',
            'Choose Interpret Original Bytes As > '+suffix+'; approve discard only in the explicit positive dirty-document case.',
            'The decoded view becomes '+repr(text+'\r\nx\n')+' using '+suffix+'; disk bytes stay identical. Cancelling the dirty-document confirmation preserves edits and original interpretation.',
            'An active document operation blocks reinterpretation; no complete original source must produce a clear refusal.',
            'Replace/truncate the captured original before the read; do not substitute new-generation bytes or silently discard unsaved edits.',encoding)
        add('encoding.convert.'+suffix,'Open UTF-8 '+repr(text+'\r\nx\n')+' with BOM off; pin original bytes.',
            'Choose Convert Save Encoding To > '+suffix+'; inspect the text, then save the owned file.',
            'Text stays '+repr(text+'\r\nx\n')+'; the selected save encoding is '+suffix+'; saved bytes are exactly '+raw+'. Encoding metadata Undo restores the previous save policy.',
            READ_ONLY, 'For legacy targets append an unrepresentable emoji and save: refuse without replacing original bytes. For Unicode targets deny the owned save destination: original bytes survive.',encoding,True)
    for suffix,target in [('lf','\n'),('crlf','\r\n'),('cr','\r')]:
        for selected in [False,True]:
            command='encoding.eol.'+('selection_' if selected else '')+suffix
            before='a\r\nb\nc\rd'
            after='a'+target+'b\nc\rd' if selected else target.join(['a','b','c','d'])
            add(command,'Open exact UTF-8 '+repr(before)+('; select byte range [0,3).' if selected else '; no selection.'),
                'Convert '+('selection' if selected else 'document')+' line endings to '+suffix+'.',
                'Exact resulting UTF-8 text '+repr(after)+'; one Undo restores '+repr(before)+' and dirty state. No autosave.',
                READ_ONLY, 'Exhaust the owned edit/staging budget; no partial EOL conversion is committed.',encoding,True)
    for suffix,expected in {
        'choose':'Encoding root popup lists current state, interpretation, conversion, BOM and EOL actions.',
        'choose_interpret':'Popup contains the 20 registered interpretation choices; choosing one dispatches its exact encoding.interpret ID.',
        'choose_convert':'Popup contains the 20 registered conversion choices; choosing one dispatches its exact encoding.convert ID.',
        'eol':'Popup contains document and selection CR/LF/CRLF actions with current context.',
        'binary':'Binary warning choices expose information, remain read-only, and explicit text-edit permission.',
        'charsets':'Searchable Character Sets picker opens; query Japanese returns Shift-JIS and EUC-JP; Escape preserves encoding and text.',
        'info':'Current encoding information remains accurate; this informational row performs no document mutation.',
        'binary.info':'Binary-like warning remains visible and accurate; the informational row performs no edit or save.',
        'failure':'The captured unrepresentable range is selected at its recorded revision and the encoding failure reason is shown.',
        'binary.readonly':'The binary-like source stays read-only; typing is rejected and its original bytes remain unchanged.',
        'binary.edit':'Explicit permission removes the binary warning read-only gate; ordinary read-only/busy guards still apply and original bytes are not rewritten until Save.',
    }.items():
        add('encoding.'+suffix,"Open a binary-like generated source for binary actions; for failure save '🙂' to Latin-1 and retain the refusal; otherwise open UTF-8 'Aé'.",
            'Use the indicated Encoding menu or picker row.',expected,
            'Missing document/state or a stale encoding failure cannot mutate or select another document.',
            'For failure, edit after the refused save and invoke the old failure: reject stale selection. Other informational/picker actions: '+PURE,encoding)
    for on in [True,False]:
        add('encoding.bom_'+('on' if on else 'off'),"Open UTF-8 'A' with the opposite BOM state; record its save target.",
            'Choose the BOM action and save the owned file.',
            'Exact saved bytes '+('efbbbf41' if on else '41')+'; text stays A and metadata Undo restores the prior BOM choice.',
            READ_ONLY+' A codec with no BOM support cannot enable a BOM.',
            'Deny owned destination replacement; previous complete file survives and save failure stays visible.',encoding,True)

    views=[APP+'views.rs']
    tabs="Open 20 distinct files named 00.txt through 19.txt containing their own two-digit name; capture workspace document order and active pane/tab IDs."
    for index in range(20):
        add('window.select.'+str(index),tabs+' Activate a different document.',
            'Choose the corresponding Window menu entry; resolve its document identity, not only its menu position.',
            f'The document at workspace index {index} ({index:02}.txt in this fixture) becomes active; its contents/dirty state are unchanged.',
            'Remove that document and resync the Window menu; a stale/out-of-range command cannot activate or mutate an unrelated document.',PURE,views)
    for suffix,expected in {
        'split_vertical':'A second pane appears with vertical orientation; active document and original bytes are preserved.',
        'split_horizontal':'A second pane appears with horizontal orientation; active document and original bytes are preserved.',
        'clone_other':'Both panes show the same document identity; editing one updates both texts while independent caret/fold/scroll state remains.',
        'move_other':'The active tab moves to the other pane without copying its document; contents, dirty state and history survive.',
        'close_split':'The split closes and all document identities/unsaved edits survive in the remaining layout.',
        'focus_other':'Active pane changes to the other pane and restores its own caret/selection; document bytes stay unchanged.',
        'sync_vertical':'Vertical synchronization toggles; scrolling maps logical lines with wraps/folds and cannot recurse or manufacture line numbers for spacers.',
        'sync_horizontal':'Horizontal synchronization toggles; only horizontal origin follows while enabled; document bytes remain unchanged.',
    }.items():
        add('view.'+suffix,tabs+" Use 200 numbered lines and different pane sizes for sync; start with two panes except for split creation.",
            'Invoke the indicated View action and inspect both pane identities/state.',expected,
            'No second pane for focus/close/sync, or no active document for clone/move: no unrelated document changes.',PURE,views)
    for suffix,expected in {
        'next':'Activate the next tab in current pane order; wrap at the end without changing bytes.',
        'previous':'Activate the previous tab in current pane order; wrap at the beginning without changing bytes.',
        'move_left':'Move the selected tab one position left within its legal pinned/unpinned group.',
        'move_right':'Move the selected tab one position right within its legal pinned/unpinned group.',
        'pin':'Toggle the selected tab pin and maintain pinned-first grouping, document identity and active selection.',
        'sort_name':'Tabs sort by display name while preserving pinned grouping and the active document identity.',
        'sort_path':'Tabs sort by full path while preserving pinned grouping and active document identity.',
        'sort_descending':'Sort descending by display name; pinned grouping and active document identity remain effective.',
        'vertical':'Tab strip orientation toggles; active document and all tab state remain.',
        'mru':'MRU selection surface follows actual recent activation order; Escape cancels without changing documents.',
        'color':'Selected tab color cycles none -> #36C9C6 -> #D19A66 -> #C678DD -> #61AFEF -> none; session restore preserves it and document bytes are unchanged.',
    }.items():
        add('view.tabs.'+suffix,tabs+' Pin 00.txt; activate 07, 02, 05 in that order; include duplicate basenames in two generated directories.',
            'Invoke the tab action from its actual tab/menu surface.',expected,
            'No active tab, or a movement at its legal group boundary: no hidden tab or pinned grouping is corrupted.',
            'Use an invalid retained layout on restart: preserve recoverable document state and report/fall back without data loss.',views)

    files=[APP+'lifecycle.rs',CORE+'workspace.rs',APP+'watch.rs']
    saved="Create owned a.txt with UTF-8 bytes 'before\\n'; open and change the buffer to 'after\\n'. Disable autosave. Capture path, document identity, dirty state, saved hash and recovery state."
    file_specs={
        'new':('Use File > New.','A new clean Untitled document accepts immediate Unicode typing; configured new-file encoding/EOL apply when first saved.',NO_DOCUMENT, 'Make profile/default configuration unavailable; use valid defaults with an honest error and keep already-open files unchanged.'),
        'open':('Use Ctrl+O and choose owned b.txt containing B.','b.txt opens with exactly B; existing a.txt edits survive.', 'Cancel the file picker: no new document or mutation.', 'Choose a deleted/inaccessible file; show failure and retain all current documents.'),
        'save':('Invoke Save on a.txt.','Disk becomes exactly after\\n; the captured ContentState becomes clean. Edit during save: the later text remains dirty.',READ_ONLY,'Deny replacement or change the captured disk fingerprint; preserve original bytes and report the conflict/error.'),
        'save_as':('Choose Save As and owned copy.txt.','copy.txt contains after\\n and becomes the document path; a.txt remains before\\n. Save state corresponds only to the captured revision.',READ_ONLY,'Cancel the picker or deny the destination; current path, buffer and original disk bytes remain unchanged.'),
        'save_copy':('Choose Save Copy and owned copy.txt.','copy.txt contains after\\n; current document remains a.txt and retains its prior dirty state; a.txt stays before\\n.', 'An incomplete source cannot produce a claimed complete copy.', 'Cancel/deny the destination; neither original nor existing copy is partially replaced.'),
        'save_all':('Also open dirty b.txt and Untitled; invoke Save All and supply an owned path for Untitled.','Each successful document has exact captured saved bytes; new edits stay dirty and errors identify the affected document.', 'While Save All is active, a second request cannot duplicate saves or prompts.', 'Deny b.txt while a.txt succeeds; retain per-document results and every unsaved buffer, with no false all-saved state.'),
        'cancel_save_all':('Start Save All with a blocked owned save, then Cancel Save All.','No new save is scheduled; already committed saves stay valid and pending documents remain dirty.', 'With no Save All active, the action is unavailable and does not cancel unrelated work.', 'Race cancellation with a completed save; retain the actual terminal result and never roll back a completed file blindly.'),
        'cancel_operations':('Start an owned long open/search/save preparation and cancel pending operations.','Outstanding owned jobs stop at supported cancellation boundaries; already durable saves remain intact.', 'No pending operation: no state or file change.', 'Cancel during queued/completing jobs; no late stale result can mutate another document.'),
        'close':('Close dirty a.txt; exercise Cancel, Save, and explicit Discard in separate reset runs.','Cancel retains the tab/edits; Save closes only after successful save; Discard closes exactly that document and retires its recovery as declared.', 'A pending exclusive operation cannot bypass the close/save decision.', 'Deny the requested save; the tab and unsaved text stay open.'),
        'restore_closed':('Close clean a.txt, then Restore Last Closed Tab.','The last closed recoverable tab reopens with its exact path/state; other tabs remain.', 'Empty closed-tab history: no fabricated tab.', 'Remove its file before restore; show failure without substituting another file.'),
        'read_only':('Toggle Set Read-Only on the active document and attempt typing; then toggle back.','On blocks edits without discarding current text; off restores ordinary edit eligibility. This flag does not rewrite file ACLs.',NO_DOCUMENT,PURE),
        'save_conflict_compare':('Generate a save conflict by replacing a.txt externally, then Compare Save Conflict Versions.','Editor and retained other-version sources are labelled with the correct paths/revisions; neither is auto-overwritten.', 'No selected retained conflict: no compare of unrelated files.', 'Remove one conflict artifact; report unavailable and preserve every remaining version.'),
        'save_conflict_next':('Retain two save conflicts and select Next Save Conflict twice.','Selection cycles through actual retained conflict identities; no disk mutation.', 'No retained conflicts: no stale selection.',PURE),
        'save_conflict_retain_other':('Generate a save conflict and choose Retain Other Save Version to an owned destination.','The chosen other version is copied byte-identically with its identity preserved; editor and target originals remain intact.', 'No retained other-version bytes: action is unavailable.', 'Deny the chosen destination or replace a retained input; refuse ambiguous copy and preserve originals.'),
        'save_conflict_save_elsewhere':('Generate a save conflict and save the editor version to a distinct owned path.','The separate path has exact editor bytes; conflicting disk version remains preserved.', 'No current selected editor conflict or incomplete source: reject.', 'Deny the distinct path; preserve both versions and keep the editor dirty.'),
        'retry_save_cleanup':('Retain a failed saved-recovery cleanup receipt; repair its owned access problem and retry.','Only the acknowledged saved state is retired, with a terminal cleanup result; newer recovery edits survive.', 'No failed cleanup receipt: no unrelated cleanup.', 'Change the protected identity or keep access denied; retain the receipt and report failure without deleting unrelated recovery.'),
        'retry_save_recovery':('Retain a failed save-recovery discovery; repair owned access and retry.','Discovery finishes against the current protected identities and exposes correct retained save conflicts.', 'No retryable discovery: no duplicate background request.', 'Discovery fails again or source identity changes; preserve existing records and show retryable error.'),
        'transcode.resume':('Pause a generated encoding conversion at its temporary disk quota; increase the approved isolated quota and resume.','The same owned conversion resumes and publishes exact complete bytes only after validation; existing document stays available.', 'No paused conversion: no new conversion or quota mutation.', 'Exhaust the new quota or change the original source; preserve original data and the explicit failure/pause state.'),
        'transcode.cancel':('Pause an owned conversion at quota exhaustion, then cancel.','Only that conversion stops; original document/file remains and owned staging is bounded/cleaned.', 'No paused conversion: no unrelated cancellation.', 'Race terminal completion with cancel; preserve actual committed data and reject stale callbacks.'),
        'reveal':('Save the owned file and choose Reveal in Explorer.','Explorer targets only the active associated local file; document bytes/state remain unchanged.', 'Untitled/no native path: unavailable with reason.', 'Remove/invalidate the path; show native shell failure without launching a different target.'),
        'terminal':('Choose Open Terminal Here for an owned associated local file.','The configured terminal opens at the file parent directory with literal path handling; no document or unrelated process is modified.', 'No allowed local working directory: reject.', 'Use a missing terminal executable or denied directory; show a clear failure, no implicit shell fallback.'),
        'recent.clear':('Populate Recent Files with 15 generated owned paths, then Clear Recent Files.','Only the recent-path list clears; current documents and disk files remain.', 'An empty recent list stays empty.', 'Deny profile persistence; retain honest error state without damaging the previous settings/profile.'),
    }
    for suffix,(action,expected,disabled,failure) in file_specs.items():
        add('file.'+suffix,saved,action,expected,disabled,failure,files)
    for index in range(15):
        add('file.recent.'+str(index),'Open and close 15 known files in a recorded order; capture the Recent Files slot-to-path table and hashes.',
            f'Invoke slot {index} from the current Recent Files menu.',
            f'Open exactly the captured path at slot {index} with its expected bytes; no path is reconstructed from a display basename.',
            'Remove that recent slot then refresh the menu; a missing slot cannot open another document.',
            'Delete or deny the captured file before activation; report failure without opening another path.',files)
    add('app.quit',saved+' Also create one dirty Untitled document.', 'Use File > Exit; test Cancel, Save and Discard choices separately.',
        'Cancel keeps the application and all unsaved documents; completed consent closes exactly once, with successful saves durable and owned processes stopped.',
        'During a pending save/close transition, repeated Exit cannot bypass consent or execute twice.',
        'Deny one requested save; preserve that document and all remaining unsaved work, with no false clean exit.',files)
    watch=[APP+'watch.rs',CORE+'workspace.rs']
    log="Create owned log.txt='one\\ntwo\\n'; open it and record its generation, disk hash and viewport. Append through a separately owned writer using Flush(true)."
    for suffix,expected in {
        'monitor.start':'Follow mode starts on the active file, appends remain continuous and the viewed source becomes protected from ordinary edits.',
        'monitor.pause':'Follow scrolling pauses; incoming bytes remain observable without jumping the viewport.',
        'monitor.resume':'Follow scrolling resumes and requests the final viewport; no bytes are skipped or duplicated.',
        'monitor.unlock':'Explicit confirmation stops following and captures a fixed editable generation; later appends do not silently join that fixed view.',
        'monitor.reopen':'The actual current file is reopened and following resumes with correct generation and prior pane; owned unsaved edits are never silently discarded.',
        'external.auto_reload':'Clean-file auto reload toggles; external changes refresh only eligible clean buffers and never discard dirty edits.',
        'external.check':'One current filesystem check is requested; changed files are reported by captured identity, without synchronous unbounded UI work.',
        'external.reload':'Confirmed dirty reload or clean reload reads the current disk version; cancelling dirty consent preserves the buffer.',
        'external.keep':'The buffer remains exactly unchanged; conflict notification resolves but Save still checks the disk fingerprint.',
    }.items():
        add('file.'+suffix,log+' Start monitoring before pause/resume/unlock/reopen; create an external rewrite before external actions.',
            'Invoke the named monitoring/conflict action; for unlock test both consent and cancellation.',expected,
            'Untitled/no monitored source or no applicable state: no unrelated file or pane changes.',
            'Rotate, truncate or rewrite the same-size source while the action runs; uncertain continuity is SourceChanged, not accepted append. Preserve owned edits.',watch)
    for suffix in ['open','reload','follow']:
        add('file.remote.'+suffix,'Use a disposable Windows lab with an owned reachable UNC share; record the literal path, permissions and network capability policy.',
            'Request remote '+suffix+' and exercise both consent and denial.',
            'Only the explicitly permitted path is read; '+suffix+' uses captured source generation and does not reinterpret a remote path as a local executable.',
            'Denied/unavailable network capability must refuse with a reason and no background fallback access.',
            'Disconnect or replace the share during reading; expose unavailable/SourceChanged, preserve owned text and allow cancellation.',watch)

    panels=[APP+'workspace_panels.rs',CORE+'workspace_panel.rs']
    workspace="Create owned root/{a/main.rs,b/main.rs,Cargo.toml}; main.rs contains 'fn main() {}\\n', Cargo.toml contains '[package]\\nname=\"qa\"\\n'. Select the root in Workspace."
    for suffix,expected in {
        'workspace':'Workspace visibility toggles; opening it closes Document List and focuses Explorer; closing returns focus to editor.',
        'documents':'Document List visibility toggles; opening hides Explorer and focuses the list; closing returns focus to editor.',
        'outline':'Outline visibility toggles and focus follows open/closed state; stale symbols from another language never appear.',
        'documentMap':'Document Map visibility toggles; logical navigation and document bytes remain unchanged.',
    }.items():
        add('view.'+suffix,workspace,'Toggle this panel twice and switch between Rust and TOML.',expected,
            'Missing optional provider/root must show an explicit empty/unavailable state rather than stale contents.',PURE,panels)
    for suffix,expected in {
        'openFolder':'Only the chosen authorized root is adopted; cancelling the folder picker preserves the previous root.',
        'loadMore':'The next bounded batch of entries loads; previously seen entries keep their identities and no duplicates appear.',
        'refresh':'The current tree refreshes from current filesystem identities; removed entries do not remain actionable ghosts.',
        'createFile':'The chosen absent owned path becomes an empty file; no existing entry is overwritten.',
        'createFolder':'The chosen absent owned path becomes a directory; no file or parent outside the owned root is replaced.',
        'rename':'After closing its open tab, selected a/main.rs moves to a/renamed.rs with identical bytes; the old path disappears.',
        'delete':'After closing its open tab, selected a/main.rs is retained for Undo and removed from its original path; unrelated files remain.',
        'undoDelete':'The last retained deletion returns byte-identically to its original path, without overwriting a newly created competing entry.',
    }.items():
        add('workspace.'+suffix,workspace+' For loadMore create >one enumeration batch; for undoDelete perform one retained deletion first.',
            'Use the selected-entry Workspace action and the exact owned destination where prompted.',expected,
            'No selected entry/undo receipt or an operation already running: no duplicate action. Renaming/deleting an open document is refused.',
            'Create a conflicting destination, deny access or substitute an owned junction before commit; reject without traversing outside the authorized root.',panels)
    for suffix,expected in [('sortName','display name'),('sortPath','full path'),('sortTabOrder','actual tab order')]:
        add('documents.'+suffix,workspace+' Open the three documents in nonalphabetical order and select the second.',
            'Choose the Document List sort mode.', 'Rows follow '+expected+' while selection remains bound to the same document identity and no bytes change.',
            'Empty Document List remains empty; no stale row becomes selected.',PURE,panels)
    for suffix in ['save','close']:
        add('documents.'+suffix,saved+' Open Document List; select dirty a.txt while another editor tab is active.',
            'Invoke '+suffix+' on the selected list row.',
            'The selected document, not the active unrelated editor, follows the ordinary '+suffix+' consent/transaction behavior; other documents remain unchanged.',
            'No selected row or stale document identity: no handler mutation.',
            'Deny the selected document save or cancel its close; preserve its edits and selected row.',panels)
    for suffix,expected in {
        'importFunctionList':'A valid owned Notepad++ functionList XML imports with a deterministic compatibility report and binds to the captured language/extension.',
        'loadDefinition':'A valid owned TOML outline definition loads for the captured language/extension; switching document cannot apply a stale result to another language.',
        'exportDefinition':'The active definition exports as TOML through complete staged replacement; parsing it recreates the same definition.',
        'cancelImport':'The active import cancels; the old valid definition remains and no partial symbols publish.',
    }.items():
        add('outline.'+suffix,workspace+' Use a small definition matching fn main and a malformed/256-KiB-plus-one counterpart.',
            'Use the Outline definition action and its owned file picker.',expected,
            'An import/export already pending or absent active definition blocks overlapping operations.',
            'Malformed/oversized input or denied destination is reported; the prior definition and existing destination remain intact.',panels)

    utility=[APP+'utilities.rs',CORE+'utilities.rs']
    for algorithm,value in [('md5','900150983cd24fb0d6963f7d28e17f72'),('sha1','a9993e364706816aba3e25717850c26c9cd0d89d'),
                            ('sha256','ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad'),
                            ('sha512','ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f')]:
        add('utilities.'+algorithm,"Open 'prefix|abc|suffix', select UTF-8 bytes [7,10); repeat with whole document 'abc'.",'Invoke this hash utility.',
            'Result reports 3 bytes and lowercase digest '+value+' for both fixtures, bound to the captured revision. No edit, disk or Undo change.',
            'Another utility pending: report that it must be cancelled before starting a new one.',
            'Cancel an owned large streaming hash or make its unread source unavailable; no partial digest is presented as complete.',utility)
    for suffix,expected in {
        'cancel':'The current utility cancellation token is signalled; it terminates without applying partial output.',
        'dismiss':'Utility result/options/popups close while document bytes and completed output remain intact.',
        'copyResult':'Clipboard receives exactly the displayed current result; the document is unchanged.',
        'statistics':"For exact text 'abc\\ndef', report 7 bytes, 7 characters, 7 graphemes, 2 words and 2 lines, with no edit.",
        'exportHtml':'Exported HTML escapes <>& and contains literal source text without executable source-controlled markup; syntax/theme colors reflect the chosen options.',
        'exportRtf':'Exported RTF retains Unicode through escaped text/code points and valid groups, using selected syntax/theme colors.',
        'print':'Print options open with selection-only false, first control focused and no job submitted until Print.',
        'printSelection':'Print options open with selection-only true; only explicitly selected logical text is eligible for submission.',
        'printNow':'Only the chosen printer receives the bounded captured job with declared header/footer/numbers/syntax/range options; no editor text is modified.',
        'printFont':'Font list opens/closes; choosing a listed family changes font_family, not document font settings.',
        'printSize':'Size list offers 8,9,10,11,12,14,16,18 pt and applies only the selected print size.',
        'printMargins':'Margin list offers 6,12,18,24 mm and applies only the selected print margin.',
        'printHeader':'Print header flag toggles; the next job reflects it without changing the document.',
        'printFooter':'Print footer flag toggles; the next job reflects it without changing the document.',
        'printNumbers':'Print line-number flag toggles; non-document spacers do not gain document line numbers.',
        'printSyntax':'Print syntax-color flag toggles; off uses plain text while preserving source characters.',
        'printRange':'Print selection-only flag toggles; empty selection cannot silently print the entire document as a selection.',
    }.items():
        add('utilities.'+suffix,"Open UTF-8 'abc\\ndef'; for export use '<script>& café 🙂'; for printing select only the second line and use an owned test printer/output.",
            'Use this Utilities result or Print Options control; capture the control before and after.',expected,
            'No result for Copy, no active job for Cancel, or no selection for selection printing: no fabricated success or unrelated mutation.',
            'For export/print, cancel or deny the owned destination/printer and preserve the original. For result/control-only actions: '+PURE,utility)

    power=[APP+'power.rs','crates/editor-surface/src/power.rs','crates/editor-surface/src/power/consumer.rs']
    transforms={
        'case.upper':('Ab é','AB É'), 'case.lower':('Ab É','ab é'),
        'case.title':('hELLO wORLD','Hello World'), 'case.invert':('Ab É','aB é'),
        'whitespace.trimStart':('  a  \n\tb\t\n','a  \nb\t\n'),
        'whitespace.trimEnd':('  a  \n\tb\t\n','  a\n\tb\n'),
        'whitespace.trim':('  a  \n\tb\t\n','a\nb\n'),
        'indent':('a\nb\n','    a\n    b\n'), 'unindent':('    a\n\tb\n','a\nb\n'),
        'tabs.toSpaces':('a\tb\n','a   b\n'), 'spaces.toTabs':('a   b\n','a\tb\n'),
        'lines.duplicate':('a\n','a\na\n'), 'selection.duplicate':('ab','abab'),
        'lines.join':('a\nb\n','a b\n'), 'lines.split':('x'*81+'\n','x'*80+'\nx\n'),
        'lines.sortAscending':('b\na\n','a\nb\n'), 'lines.sortDescending':('a\nb\n','b\na\n'),
        'lines.sortIgnoreCase':('b\nA\n','A\nb\n'), 'lines.sortNumeric':('10\n2\n','2\n10\n'),
        'lines.removeDuplicates':('a\nb\na\n','a\nb\n'),
        'lines.removeConsecutiveDuplicates':('a\na\nb\na\n','a\nb\na\n'),
        'lines.removeEmpty':('a\n\n \nb\n','a\n \nb\n'),
        'lines.removeBlank':('a\n\n \nb\n','a\nb\n'),
    }
    for suffix,(before,after) in transforms.items():
        add('editor.'+suffix,'Create a clean UTF-8/LF document with exact text '+repr(before)+'; select the whole text, tab width=4, no rectangle or composition.',
            'Invoke the indicated edit once and wait for its acknowledged transaction.',
            'Entire buffer equals '+repr(after)+'; dirty is true, saved bytes are unchanged; one Undo restores '+repr(before)+' and clean state, and Redo restores the transformed text.',
            READ_ONLY+' Active IME preedit also rejects power edits.',
            'Exhaust the declared staging/history budget or cancel before publication; no partial output. '+STALE_EDIT,power,True)
    for suffix,expected in [('moveUp','b\\na\\nc\\n'),('moveDown','a\\nc\\nb\\n')]:
        add('editor.lines.'+suffix,"Open 'a\\nb\\nc\\n' and select only b's logical line (bytes [2,4)).",'Move the selected line in the indicated direction once.',
            'Buffer becomes '+expected+'; one Undo restores a\\nb\\nc\\n and original selection.',READ_ONLY,
            'Move at the first/last legal boundary, then exhaust staging budget: no unrelated line is lost or duplicated.',power,True)
    for suffix,expected in {
        'bookmark.toggle':'Bookmark on the current line toggles without editing text; repeat removes the same bookmark.',
        'bookmark.clear':'All bookmarks clear; source bytes and dirty state stay unchanged.',
        'bookmark.next':'Caret navigates to the next bookmarked logical line and wraps according to the current bookmark list.',
        'bookmark.previous':'Caret navigates to the previous bookmarked logical line without editing.',
        'bookmark.selectLines':'Selections cover only bookmarked logical lines, preserving complete Unicode/EOL boundaries.',
        'bookmark.copyLines':"Clipboard equals 'a\\nc\\n'; document bytes and dirty state are unchanged.",
        'bookmark.cutLines':"Clipboard equals 'a\\nc\\n' and document becomes 'b\\n'; one Undo restores the complete original.",
        'bookmark.deleteLines':"Document becomes 'b\\n'; one Undo restores 'a\\nb\\nc\\n'; clipboard is unchanged.",
    }.items():
        add('editor.'+suffix,"Open 'a\\nb\\nc\\n', bookmark a and c and place caret on b; use no bookmark on b for toggle.",
            'Invoke this bookmark action.',expected,READ_ONLY if suffix.endswith(('cutLines','deleteLines')) else 'With no applicable bookmarks, navigation/copy/select produces no fabricated range or edit.',
            'For clipboard actions force clipboard access failure: do not cut until copy succeeds. For mutation actions reject stale/budget-limited plans; preserve bytes.',power)
    for suffix,expected in {
        'caret.above':'A second caret appears one logical row above at the same display column, clamped by the shorter row.',
        'caret.below':'A second caret appears one logical row below at the same display column, respecting tabs and wide glyphs.',
        'selection.nextOccurrence':'The next exact occurrence of the selected token is added without losing earlier selections.',
        'selection.allOccurrences':'All nonoverlapping exact occurrences are selected once, within the selection-count/work budget.',
        'selection.skipOccurrence':'The next occurrence is skipped while earlier accepted selections remain.',
        'selection.undoOccurrence':'Only the last occurrence-selection expansion is reversed; text Undo history is unchanged.',
        'selection.rotatePrimary':'Primary selection rotates through existing carets without changing the selected ranges.',
        'selection.expandLines':'Each selected span expands to complete logical line boundaries without overlapping duplicate edits.',
        'selection.escape':'Secondary selections/rectangle state cancel according to editor Escape precedence; the primary caret and document remain.',
        'lines.hide':'Selected lines b/c hide while line a remains a visible anchor; buffer and saved bytes stay identical.',
        'lines.showAll':'Manual hidden ranges clear; all original lines become visible without changing fold policy or text.',
    }.items():
        add('editor.'+suffix,"Open 'a token\\nb token\\nc token\\n'; select the first token; for carets place at display column 2 on b; for hide select b/c.",
            'Invoke this selection/view action and capture every range and primary index.',expected,
            'Missing eligible occurrence/row or exhausted selection budget: no invalid range, duplicate caret or text mutation.',
            'Change the captured source before asynchronous selection completion; reject stale ranges. '+UI_ONLY,power)
    for suffix in ['toggleLine','toggleBlock']:
        add('editor.comment.'+suffix,"Use a Rust document 'let x = 1;'; record its exact selection and a separate provider-absent plain-text fixture.",
            'Invoke the comment toggle twice; test empty and nonempty selection.',
            ('Line toggle inserts/removes the registered // marker at eligible line starts.' if suffix=='toggleLine' else 'Block toggle inserts/removes registered /* and */ around the chosen range.')+' The second toggle and one-step Undo restore exact bytes; provider-absent state reports unavailable without mutation.',
            READ_ONLY+' A missing CommentProvider has an explicit reason, including the pre-PR-013 contract fixture.',
            'Reject stale provider/revision or an excessive marker argument before any edit; preserve the original.',power,True)
    add('editor.clipboard.toggleHistory',"Use a clean profile with history disabled; copy 'A', enable history, then copy B and C.",'Toggle history, inspect available entries, then restart.',
        'Only text copied while enabled is retained within configured entry/byte limits; disabling clears availability and no content history persists after restart.',
        'Unwritable settings cannot claim persistent toggle success.', 'Exceed 20 entries and 16 MiB; bounded eviction occurs, never unbounded growth or document mutation.',power)
    for suffix in ['paste.plainText','paste.fromHistory','rectangle.paste','rectangle.delete','column.insert']:
        add('editor.'+suffix,"Use the exact fixtures in tests/e2e/column_fixture.py for columns; otherwise 'ab\\ncd\\n', clipboard X, and a rectangle covering column 1 of both lines.",
            'Open the Column Editor/History panel when required; choose explicit text X (or numbers start=1,step=1,width=2,base=10,repeat=1). For plain paste use a normal selection.',
            'Rectangle paste replaces each selected cell with X; rectangle delete removes only those cells. Column insertion pads short rows and matches column_fixture. History paste uses the selected retained entry, not the current clipboard. Each edit is one Undo transaction.',
            READ_ONLY+' Missing rectangle/history, disabled history, or invalid numeric base/count is an explicit refusal.',
            'Change target revision after the panel opens or provide oversized/invalid arguments; preserve the original and reject stale insertion.',power,True)
    edit_sources=[CORE+'macros.rs','crates/editor-surface/src/lib.rs','crates/editor-surface/src/power/consumer.rs']
    for suffix,setup,expected in [
        ('insert_text',"Open 'ab', caret byte 1; normalized recorded argument text=Ω.","Text becomes 'aΩb'; one Undo returns 'ab'."),
        ('backspace',"Open 'a🙂b', caret immediately after the emoji.","Text becomes 'ab' without splitting the emoji; one Undo restores it."),
        ('delete',"Open 'a🙂b', caret immediately before the emoji.","Text becomes 'ab' without splitting the emoji; one Undo restores it."),
        ('copy',"Open 'aΩb', select UTF-8 [1,3).",'Clipboard becomes Ω; bytes, dirty state and Undo are unchanged.'),
        ('cut',"Open 'aΩb', select UTF-8 [1,3).",'Clipboard becomes Ω and buffer ab; one Undo restores aΩb.'),
        ('paste',"Open 'ab', caret 1, clipboard Ω.",'Buffer becomes aΩb in one transaction; Undo restores ab.'),
        ('select_all',"Open 'aΩb'.",'Selection covers UTF-8 [0,4), with complete character boundaries and no text edit.'),
        ('undo',"Open clean 'ab', insert Ω at byte 1 and await acknowledgement.",'One Undo restores ab and the saved clean state; no comparison of monotonic revision IDs is used.'),
        ('redo',"Open clean 'ab', insert Ω at byte 1 and Undo once.",'Redo restores aΩb and dirty state, preserving the correct caret/selection metadata.'),
    ]:
        add('edit.'+suffix,setup,'Invoke the normal input/clipboard/history route. Internal normalized commands are exercised by real input or recorded playback with explicit arguments.',expected,
            READ_ONLY if suffix in {'insert_text','backspace','delete','cut','paste','undo','redo'} else NO_DOCUMENT,
            'Clipboard failure, cancelled preedit or exhausted edit/history budget cannot cause partial mutation; a failed Cut must preserve text.',edit_sources)
    for suffix,caret in [('move_left',3),('move_right',5),('move_up',1),('move_down',7),('move_home',3),('move_end',5)]:
        add('edit.'+suffix,"Open 'ab\\ncd\\nef', caret UTF-8 offset 4 with no selection and no wrap.",
            'Press the corresponding unmodified navigation key, retaining its normalized command ID.',
            f'Caret becomes UTF-8 offset {caret}; document bytes, dirty state and Undo history are unchanged.',
            'At the relevant document boundary the caret clamps to a valid boundary; no invalid range.',PURE,edit_sources)

    language=[APP+'language.rs',CORE+'language.rs',CORE+'language_catalog.rs']
    for suffix,expected in {
        'choose':'Language picker opens; choosing Rust sets the active document override and Escape leaves its previous language intact.',
        'udl.import':'The valid qaudl definition installs atomically and styles its captured target; invalid input leaves the prior durable definition unchanged.',
        'udl.edit':'A new JSON definition editor opens with the current definition; the source document is not replaced and only a validated revision may become active.',
        'udl.export':'Export is complete valid definition JSON at the owned chosen path; reimport yields equivalent definition fields.',
        'udl.preview':'A new document contains the definition keywords, optional sample comment and operators, styled by that exact definition; original documents remain untouched.',
        'signatures.import':'Validated static signatures bind to the captured language/UDL ID, with no execution of imported data.',
    }.items():
        add('language.'+suffix,'Use tests/e2e/udl_fixture.py sample/definition plus malformed and oversized counterparts; open two different-language documents.',
            'Use this Language action and its owned file/choice dialog.',expected,
            'No definition for edit/export/preview or storage reconciliation in progress: no stale or partial publication.',
            'Malformed/oversized import, denied export, or a changed target revision preserves the previous definition and original file; error is visible.',language)
    add('editor.completion.show',"Open Rust 'let alpha = 1;\\nalp' with caret after alp; create another document containing unrelated symbols.",
        'Request completion, then accept alpha; reset and type another character while suggestions are computing.',
        'Eligible current-language/current-revision alpha is offered; acceptance replaces only the intended prefix. Old suggestions cannot replace later typing; strings/comments obey supported syntax policy.',
        'Unavailable/incomplete source syntax reports preparation/unavailable rather than accepting stale suggestions.',
        'Cancel, close the source or change language during completion; late results cannot mutate another document.',language,True)
    for suffix in ['all','unfoldAll','toggleCurrent']+[f'level{n}' for n in range(1,9)]:
        expectation={'all':'Collapse all known fold regions at every level.', 'unfoldAll':'Expand every known fold and cancel outstanding fold targeting.',
                     'toggleCurrent':'Toggle only the current known fold and cancel pending fold targeting.'}.get(suffix,
                     'Collapse known regions at requested level '+suffix.removeprefix('level')+'; apply the retained target as additional indexing completes.')
        add('view.fold.'+suffix,'Use a generated Rust file with nine nested brace regions, known source offsets and a second document. Repeat with incomplete large-file indexing.',
            'Place caret on the third nesting header and invoke the fold action.',expectation+' Hidden state is view-only; source bytes, saved output and Undo are unchanged.',
            'No eligible known fold/provider or a changed target document: no invented fold ranges.',
            'Edit or switch document during indexing; obsolete fold results must be rejected and input remain responsive.',language)

    macros=[APP+'macros.rs',CORE+'macros.rs','tests/e2e/macro_fixture.py']
    macro_setup='Use the actual macro fixture and expected bytes in tests/e2e/macro_fixture.py; record explicit text/search arguments in a clean isolated profile. Capture library names, selected slot and the replay target.'
    for suffix,expected in {
        'record':'Recording starts; only acknowledged stable command IDs, explicit text and resolved search arguments are recorded, excluding panel-only state.',
        'stop':'After outstanding edits acknowledge, recording ends under the next unused Macro N name; no pending edit is lost and library becomes dirty.',
        'play':'Selected macro runs once against its captured target and produces macro_fixture.FINAL from INITIAL, without recording its own playback.',
        'play_eof':'A macro containing forward navigation repeats until EOF; no-progress or invalid events stop honestly rather than looping indefinitely.',
        'play_n':'Manager count 3 runs exactly three repetitions; 0, 10001 and nonnumeric values are rejected.',
        'cancel':'Active playback cancels without starting more events; already acknowledged edits remain undoable.',
        'resume':'After fixing the declared failed prerequisite, replay resumes at the recorded failed location without repeating already acknowledged events.',
        'manager':'Macro Manager opens with current library selection and real enabled/hidden controls; document content is unchanged.',
        'manager_close':'Manager closes in idle, recording, playback and file-operation states without changing the underlying operation or document.',
        'rename':'Selected macro renames to QA renamed; duplicate/empty name is rejected and no other macro is overwritten.',
        'ghost':'Typing delay 25 ms persists for the selected macro; values outside 0–60000 ms are rejected.',
        'shortcut':'Assign the chosen nonconflicting shortcut to the stable saved-macro slot; collision validation preserves the prior binding.',
        'save':'The current macro library persists through owned atomic files; restart reloads equivalent stable events and arguments.',
        'reload':'After repairing a failed library read, load succeeds without dropping current valid state; already-loaded storage reports that no retry is needed.',
        'import':'A valid version-1 macro imports with stable validated events; no imported command executes during import.',
        'export':'Selected macro exports exact stable events and arguments to a valid version-1 file; roundtrip reimport is equivalent.',
    }.items():
        add('macro.'+suffix,macro_setup+' For manager actions open the manager first and set the named field; for resume create one recoverable failed event.',
            'Invoke the named Macro menu/manager action.',expected,
            'Absent selected macro, unfinished profile reconciliation, failed storage or incompatible active operation gives an explicit reason; no hidden mutation.',
            'Malformed import, missing command, denied library write or stale replay target stops with retained error/location; no partial file replacement or replay into another document.',macros)
    for slot in range(1,33):
        add(f'macro.saved.{slot:02}',macro_setup+f' Populate all 32 slots with unique literal inserts S01 through S32; slot {slot:02} contains only literal S{slot:02}.',
            'Use the actual saved-macro menu/assigned shortcut for this slot on an empty writable document.',
            f'Exactly S{slot:02} is inserted once and is undoable; the slot identity and explicit argument survive restart.',
            'Empty/nonexistent slot, absent target or read-only document cannot invoke another macro.',
            'Revoke/remove the selected macro or change the target during queued playback; fail without using a different slot/target.',macros,True)
    for suffix,expected in {
        'prompt':'Run dialog opens and takes text focus; Escape cancels without starting a process.',
        'load':'The owned version-1 direct command definition loads without execution; argv and mode are retained literally.',
        'execute':'Only the explicitly approved direct executable starts; observed argv equals macro_fixture.ARGS and cwd is the owned scratch directory, with no shell expansion.',
        'cancel':'Both owned parent and descendant terminate; unbounded output stays within the configured cap and the editor remains usable.',
    }.items():
        add('run.'+suffix,'Use macro_fixture.external_definition and external_fixture.py with spaces, quotes, shell metacharacters, CJK/non-BMP and empty arguments.',
            'Use the Run dialog/loaded-command action and explicit consent as applicable.',expected,
            'No loaded definition, denied execution consent or existing incompatible job prevents launch.',
            'Missing executable, failed process creation or cancellation during unlimited output is reported with bounded owned cleanup; no implicit shell fallback.',macros)
    for suffix,expected in {
        'clear':'Retained output clears without altering source documents or terminating an unrelated process.',
        'close':'Output surface closes, pending location work cancels and editor focus returns; running process ownership is retained.',
        'copy':'Clipboard receives exactly the retained output text, including truncation indicators when present.',
        'save':'Chosen owned path receives exactly retained output through complete atomic replacement.',
        'open_link':'Selected generated path:line:column opens the literal authorized file and navigates to that location; unrelated paths are not inferred.',
    }.items():
        add('output.'+suffix,'Run the owned external fixture and retain its known output plus a second generated diagnostic location.',
            'Select the appropriate output row and invoke this Output control.',expected,
            'No selected location for open_link reports Select a path:line:column output location first; no stale target.',
            'Deny clipboard/file/path access or change output generation during navigation; report failure and preserve unrelated files and document state.',macros)

    search=[APP+'search.rs',APP+'search/replace.rs',CORE+'find.rs',CORE+'workspace.rs']
    query="Open a.txt='one ONE stone\\none\\n' and b.txt='one\\n'; use literal query one, replacement X, case-sensitive=false, whole-word=false. Capture original UTF-8 offsets and revisions."
    for suffix,expected in {
        'find':'Find field opens and owns text focus; entering one produces current results without editing the document.',
        'replace':'Find/Replace opens with a separate replacement field; editing the field alone changes no document bytes.',
        'find_next':'Next exact match is selected at its original UTF-8 range; repeated invocation advances/wraps only through current results.',
        'find_previous':'Previous exact match is selected at its original UTF-8 range with the declared wrap state.',
        'replace_one':"Only the current confirmed match becomes X; one Undo restores its original case and bytes.",
        'replace_all':"With the reset default query, a.txt becomes 'X X stX\\nX\\n' in one transaction; b.txt is unchanged for current-document scope.",
        'cancel':'Current in-document search cancels; incomplete/partial results cannot enable replacement.',
        'cancel_panel':'Current panel/folder/open-document search stops, retaining an honest cancelled/partial state.',
        'close_find':'Find transient UI closes, preedit cancels and editor focus returns without changing text.',
        'close_panel':'Search results panel closes; document bytes and owned search cancellation semantics remain intact.',
        'open_documents':'Scope becomes open documents; both a.txt and b.txt results retain their own document/revision identities.',
        'folder':'The chosen owned folder scope opens with the captured query; cancelling the picker keeps the previous scope.',
        'scope.current':'Selection restriction clears and scope becomes the active document; Find opens with the same query.',
        'scope.selection':'Captured active selection becomes the exact scope; matches outside it are excluded and Find opens.',
        'match_case':'Case sensitivity toggles; ONE disappears from lowercase one results only while enabled, without changing source bytes.',
        'whole_word':'Whole-word toggles; the one inside stone is excluded only while enabled.',
        'mode':'Search mode cycles through the three supported modes; mode, field interpretation and presented label remain consistent.',
        'mode.literal':'Literal mode is selected directly; query a.b matches only a.b, not axb.',
        'mode.extended':'Extended mode is selected directly; query \\n finds real newlines and preserves literal search semantics for ordinary characters.',
        'mode.regex':'Regex mode is selected directly; query a.b matches a.b and axb, with incomplete-context limitations exposed.',
        'goto':'Go To accepts an explicit valid line/column and moves the caret there without changing the text; invalid/out-of-range input is validated.',
    }.items():
        add('search.'+suffix,query+' For mode checks use a separate a.b/axb/newline fixture; for selection scope select only the final one; for Go To request line 2 column 1.',
            'Invoke the named Search action through the current UI and wait for the reported terminal state.',expected,
            (READ_ONLY if suffix.startswith('replace_') else 'No current document/selection/results or incomplete/stale results cannot cause navigation/replacement into another revision.'),
            'Use invalid regex (, unsupported partial context, cancel, or edit the target during work; report the exact failure/completeness and preserve bytes on rejected replacement.',search)
    for style in range(1,6):
        add('search.mark.style'+str(style),query+' Complete the current search and record all returned ranges.',
            'Apply mark style '+str(style)+'.',
            'Exactly the completed current-result ranges receive style '+str(style)+'; other styles and document bytes remain unchanged.',
            'Incomplete/stale results report Complete the current search before marking; no old ranges are applied.',
            'Change source revision before dispatch or exceed mark budget; reject and preserve previous marks/text.',search)
        add('search.mark.clearStyle'+str(style),query+' Apply two different mark styles including '+str(style)+'.',
            'Clear only style '+str(style)+'.',
            'Only style '+str(style)+' ranges disappear; all other marks, bytes and dirty state remain.',
            'Absent style is a no-op with no source mutation.',PURE,search)
    add('search.mark.clearAll',query+' Apply all five styles.', 'Clear all search marks.',
        'All search-style ranges disappear; bookmark/selection state and source bytes remain unchanged.',
        'No active document: no stale-document mutation.',PURE,search)
    replacement=query+' Create a disk-only c.txt containing one, make b.txt dirty, choose the owned folder, and prepare a reviewed preview with originals/fingerprints retained.'
    for suffix,expected in {
        'replaceInFiles':'A read-only preview covers eligible chosen-folder disk files and their open documents, with no disk writes before Apply.',
        'replaceInWorkspace':'Preview additionally includes eligible unsaved Untitled workspace documents; each row is bound to its own revision/fingerprint.',
        'replacePreview.refresh':'A fresh read-only preview is prepared for the retained Files/Workspace mode; old selections/results cannot authorize unseen writes.',
        'replacePreview.apply':'Only reviewed selected rows commit. Disk originals have unique backups/durable receipts; dirty open buffers use revision-checked transactions and linked Undo.',
        'replacePreview.toggleAll':'With any rows selected, all become unselected; with none selected, all become selected. No files change.',
        'replacePreview.cancel':'Preparation, paged staging, Apply or Rollback stops at the next safe commit boundary; queued unsubmitted files are recorded skipped.',
        'replacePreview.close':'Idle preview closes; a busy preview cannot hide/abandon its active commit accounting.',
        'replacePreview.rollback':'Only unchanged committed targets restore from verified retained backups; changed targets are skipped for review, never blindly overwritten.',
        'replacePreview.preserveCase':'Preserve-case toggles and invalidates the old preview; refreshed replacements match documented source-case policy.',
        'replacePreview.includeBinary':'Binary inclusion toggles and invalidates the old preview; binary files participate only after explicit enable and fresh review.',
        'replacePreview.backups':'Backup-disable option toggles and invalidates the old preview; actual backup policy is visible before a fresh reviewed Apply.',
    }.items():
        add('search.'+suffix,replacement,'Use the indicated preview control; reset and re-review after changing options.',expected,
            'Busy, stale, incomplete or empty-selected preview cannot Apply; busy options cannot mutate in-flight policy.',
            'Externally replace c.txt after preview or edit dirty b.txt before commit; reject/skip changed identities without re-matching unseen content. Inject denied backup/receipt write to verify originals survive.',search)

    settings=[APP+'settings.rs',APP+'shortcuts.rs',CORE+'settings.rs']
    settings_setup='Use an isolated writable profile and owned workspace. Record original settings/keymap bytes; open Settings and select editor.font.size with initial 11 pt.'
    for suffix,expected in {
        'open':'Settings opens with installed-font choices and the registered toolbar command catalogue; the command palette closes.',
        'close':'Settings dismisses and editor focus returns; pending validated persistence is not silently lost.',
        'change':'Enter applies the focused setting control; font size 12 is previewed/stored in points, not scaled pixels.',
        'retry':'After fixing a failed settings write, Retry saves the current valid pending settings exactly once.',
        'revert':'Unsaved settings changes revert to the last durable state and the live preview follows it.',
        'external_reload':'The observed external settings revision is reloaded; validation applies before it changes the preview.',
        'external_keep':'Current settings remain selected after an external change; any eventual write still uses the explicit conflict policy.',
        'reset_section':'Reset confirmation targets only the selected section; cancel preserves all values and acceptance restores its defaults without resetting unrelated sections.',
        'copy_key':'Clipboard receives the exact stable selected key editor.font.size, not its translated display label.',
        'keymap_import':'Valid versioned keymap imports only known commands and validated bindings; invalid import preserves the old active/durable keymap.',
        'keymap_export':'Owned destination receives exact current versioned bindings; reimport roundtrips without changing active shortcuts during export.',
        'keymap_open':'The real owned keymap path opens as a document; an absent file is created atomically from the current map, never overwriting an existing file.',
        'shortcuts':'Keyboard Shortcuts opens with stable command IDs, current binding selection and context; palette closes.',
        'shortcut_apply':'Chosen unused Ctrl+Alt+Shift+F12 binds to the selected known command; conflict/invalid/reserved bindings produce validation instead of silently replacing another command.',
        'shortcut_close':'Shortcut editor closes without changing bindings or executing the selected command.',
    }.items():
        add('settings.'+suffix,settings_setup+' Use a valid exported keymap and malformed/duplicate-binding counterpart; use confirmed external-edit and failed-write states for their recovery actions.',
            'Use the named Settings/Shortcuts control and supplied value; test confirm and cancel where a confirmation is shown.',expected,
            'No selected key/binding, busy keymap I/O or disallowed workspace scope must refuse with a reason.',
            'Write malformed TOML, deny owned persistence, or attempt workspace override of execution/network/extension grants; reject while preserving the prior valid durable file and live configuration.',settings)

    extension=[APP+'extensions.rs',APP+'extensions/authority.rs']
    extension_setup='Use a configured diagnostic candidate and explicit signed offline runtime/catalog/packages matching its pinned release/catalog/root keys and publisher. Use an owned profile; retain every signature, digest, permission choice and host PID/creation identity. Fixture identities are never shipping identities.'
    for suffix,expected in {
        'manage':'Extension Manager opens without starting a host or granting capabilities.',
        'close':'Manager closes while owned operations remain correctly tracked; closing grants no permission.',
        'cancel':'Current manager/invocation cancellation completes; queued or staged edits do not publish after cancellation.',
        'catalog':'The selected signed offline catalogue is verified and listed only after identity/version/expiry/root policy checks.',
        'runtime_catalog':'The selected signed runtime executable and adjacent metadata are verified, then installed under the owned profile; wrong publisher/hash never executes.',
        'install':'Only the selected verified package installs atomically; capabilities remain unapproved until explicit review.',
        'permissions':'The selected installed package ID and exact requested capabilities appear for review; no grants are added yet.',
        'approve':'Only the same selected package whose permissions were reviewed receives the explicit approved grants; changing selection invalidates that consent.',
        'disable':'The selected package loses grants, owned work is cancelled, installed bytes remain, and queued/cached invocations cannot run.',
        'remove':'Only the selected verified owned package files and records are removed; unrelated documents/runtime remain.',
        'remove_runtime':'Only verified owned runtime files are removed after owned host cleanup; installed extension packages and user documents remain.',
        'next_command':'Selected command advances modulo the selected package manifest command list; no command executes.',
        'run_selected':'The exact selected package/command runs under the interactive budget with validated grants and captured target revision; resulting edits are atomic and undoable.',
        'run_background':'The exact selected package/command runs under the explicit background budget; editor remains responsive and cancellation/limits still apply.',
    }.items():
        add('extensions.'+suffix,extension_setup,'Use the corresponding Extension Manager control; panel-only actions are not invented global menu commands.',expected,
            'Missing trust/runtime/catalog/selected package, pending profile reconciliation or unreviewed grants must fail closed with an explicit reason.',
            'Tamper package/signature/hash, expire/revoke authority, race selection/cancellation, or kill only the owned host. No partial install, stale edit or revoked queued invocation is accepted.',extension)
    extension_cases={
        'ext.json.format':('JSON {"n":9007199254740993,"e":1e+30,"a":[1,2]}','Valid formatted JSON preserves exact number/exponent lexical policy and values; one Undo restores source bytes.'),
        'ext.json.minify':('JSON { "a" : [1, 2] }','Exact minified text {"a":[1,2]}; one Undo restores whitespace and source bytes.'),
        'ext.json.validate':('Valid {"a":1}, then malformed {"a":}','Validation distinguishes valid/error with correct source location; no document edit.'),
        'ext.json.tree':('JSON {"a":[true,null,3]}','A bounded tree represents object a, array indices 0/1/2 and true/null/3 with correct target identity; no text edit.'),
        'ext.xml.format':('XML <root><item>é</item></root>','Formatted XML preserves the same element/text structure and Unicode, with one Undo restoring exact original bytes.'),
        'ext.xml.validate':('Valid <root/>, then mismatched <root></wrong>','Validation distinguishes valid/error without document mutation and without network access.'),
        'ext.xml.xpath':('XML <root><item>A</item><item>B</item></root>, query /root/item','Results identify exactly both item elements in document order; unsupported XPath reports unsupported, never an empty success.'),
        'ext.hex.open':('Owned UTF-16LE BOM bytes ff fe 41 00; make an unsaved edit to B','Original-byte Hex displays ff fe 41 00 from the pinned original generation, with unsaved text edits explicitly excluded; no accidental edited-byte preview.'),
    }
    for command,(fixture,expected) in extension_cases.items():
        add(command,extension_setup+' Fixture: '+fixture+'.', 'Invoke this exact owned contribution through its actual menu/palette route.',expected,
            'Disabled/revoked package, absent verified runtime or missing required grant prevents execution and leaves the document unchanged.',
            'Reject malformed input, oversized frame, CPU loop, DTD/entity/XInclude, stale target and host crash; no partial edit or network fallback.',extension)
    add('internal.dynamic.invoke',extension_setup+' Install two signed packages contributing distinct commands and capture their composed runtime identities.',
        'Invoke a real dynamic menu item carrying its owner and contribution identity; this internal routing ID is not a standalone user action.',
        'Exactly the selected current contribution dispatches once to its owner; validate output with its concrete ext.* procedure and capture the composed inventory ID.',
        'Missing payload, removed owner, changed generation or revoked grant prevents invocation; another contribution is never substituted.',
        'Queue then revoke/replace the owner before execution; reject stale authority and leave the document unchanged.',extension)

    recovery=[APP+'recovery.rs','crates/file-io/src/recovery/mod.rs']
    recover_setup='In an owned disposable Windows profile, retain a saved document and an Untitled document with acknowledged durable edits; keep exact original/recovered text and provenance, journal/checkpoint identities and process creation records.'
    for suffix,expected in {
        'open':'Recovery Center lists only eligible non-live protected documents/checkpoints with honest Complete/Edits-only status; live adopted checkpoints are excluded.',
        'open_folder':'Only the owned recovery storage directory opens through the native shell; no recovery content is deleted or modified.',
        'restore_latest':'The latest valid durable checkpoint restores to its correct document identity; incomplete recovery remains explicitly Edits-only and cannot be saved as a falsely complete original.',
        'retry':'After repairing the owned discovery failure, a new current-root discovery runs; stale-root results cannot replace the visible list.',
        'save_as':'Selected recovered content saves to a distinct owned path after completeness and identity checks; original protected material remains available until acknowledged retirement.',
    }.items():
        add('recovery.'+suffix,recover_setup,'Use the Recovery Center/menu action with the corresponding selected eligible row.',expected,
            'No eligible selection, live owner, busy exclusive recovery operation or unsupported incomplete save must refuse explicitly.',
            'Truncate/corrupt the last record, deny access, change roots or kill at a declared durable boundary; recover only validated records and preserve original data.',recovery)
    migration=[APP+'migration.rs']
    for suffix,expected in {
        'review':'Supported imported preferences and local document paths appear in a deterministic review report; nothing is applied or executed yet.',
        'apply':'Only approved allowlisted preferences migrate into the isolated profile; unknown/execution/network/grant settings are excluded with reasons.',
        'open_paths':'Only reviewed permitted local paths open; network/unsupported paths do not trigger implicit access.',
        'cancel':'Pending import/review cancels and original profile/documents remain unchanged.',
    }.items():
        add('migration.'+suffix,'Use owned Notepad++ XML with tab width=4, spaces=true, one local file and one UNC path; include malformed/DTD and oversized variants.',
            'Use the migration review action, retaining explicit selection/consent.',expected,
            'No valid review, profile storage reconciliation or incompatible operation blocks applying stale preferences.',
            'Malformed XML/DTD, denied profile write or revoked path authorization must preserve prior valid configuration and files.',migration)
    add('profile.migration.retry','Use a failed isolated legacy-profile reconciliation with both original and candidate profile inventories retained.',
        'Repair the declared owned access problem and retry profile migration.',
        'Reconciliation reruns with exact source/destination ownership; valid existing files win according to migration policy and active document state survives.',
        'No retryable migration or one already running prevents overlapping writes.',
        'Inject a conflicting file or keep access denied; retain original/candidate bytes and explicit retry state instead of silently merging corruption.',[APP+'launch.rs'])
    update=[APP+'update.rs','apps/update-helper/src/main.rs','crates/platform-windows/src/update.rs']
    for suffix,expected in {
        'check':'Explicit update check validates the selected channel, expiry/version floors, publisher and signed bytes; no application file changes during checking.',
        'apply_on_exit':'Only a completely verified staged update is scheduled; activation occurs through the owned helper at clean Exit with durable rollback information.',
        'cancel':'Active update preparation cancels without replacing the working application; acknowledged committed state is not blindly removed.',
        'discard':'Only the verified owned staged update is discarded; working install, user profile and unrelated files remain intact.',
    }.items():
        add('update.'+suffix,'Use disposable standard-user Windows VM snapshots and explicitly signed configured baseline/update assets, exact inventories and public authority metadata.',
            'Use the actual editor/helper update path; retain before/after application and profile hashes.',expected,
            'Preview/unconfigured trust, absent staged update or pending incompatible operation prevents activation.',
            'Wrong publisher/hash/channel, expiry/rollback, reparse/traversal or observed activation failure must preserve a working original or verified rollback and durable receipts.',update)
    surface=[APP+'dock.rs',APP+'toolbar.rs','apps/bareline/src/windows_app.rs']
    for suffix,expected in {
        'bottom_panel.search':'Existing Search dock surface activates with its retained results/query and focus.',
        'bottom_panel.compare':'Existing Compare dock surface activates without recreating or mutating its source documents.',
        'bottom_panel.output':'Existing Output dock surface activates with retained output and focus.',
        'bottom_panel.close':'Dock collapses and its focus deactivates; individual operation state remains tracked.',
        'command_palette':'Command Palette opens with only applicable registered contributions and stable IDs; Escape restores editor focus.',
        'theme.cycle':'Resolved preview theme alternates Light/Dark and all open overlays update from the same tokens; source bytes stay unchanged.',
        'toolbar_toggle':'Toolbar visibility toggles; document state is unchanged and layout/hit targets update together.',
        'toolbar_customize':'Toolbar customization opens using the stable command catalogue; editing chips does not execute their commands.',
        'toolbar_focus':'Visible toolbar obtains keyboard focus on an eligible control; arrows/Tab and Escape follow its focus policy.',
    }.items():
        add('view.'+suffix,'Open two generated documents; populate Search, Compare and Output once and retain current panel/focus state.',
            'Use the named View/Dock/Toolbar action.',expected,
            'Unavailable dock surface or hidden/nonfocusable toolbar cannot acquire stale focus or execute an unrelated command.',PURE,surface)
    shell=['apps/bareline/src/windows_app.rs']
    for suffix,expected in {
        'toggle':'Tray integration preference toggles through the supported opt-in path; no new service or scheduled task is installed.',
        'hide':'Owned editor hides to the enabled tray without exiting or losing unsaved documents; a tray affordance remains available to restore it.',
        'restore':'Owned editor becomes visible with the same document and state; another application is not terminated or reconfigured.',
    }.items():
        add('tray.'+suffix,saved+' Use only a disposable/available desktop; record tray preference and owned window identity.',
            'Use this tray control and restore the owned editor at the end.',expected,
            'Disabled/unsupported tray integration cannot hide the only window irretrievably.',
            'Simulate failed tray icon registration/native shell availability; retain a usable visible editor and report failure.',shell)
    add('help.about','Launch the pinned candidate in an owned profile and record its actual executable digest/build version.',
        'Open About from Help, inspect it, then dismiss.',
        'Version/build identity, license and product information match the actual candidate; dismiss restores focus without changing documents.',
        'Do not expose internal panel-only actions as About commands.',PURE,['apps/bareline/src/windows_app.rs'])
    return rows


def bindings():
    result={}
    hashes={}
    for command,row in catalog().items():
        sources=[]
        for path in row['sources']:
            if path not in hashes:
                hashes[path]=hashlib.sha256((ROOT/path).read_bytes()).hexdigest()
            sources.append(dict(path=path,sha256=hashes[path]))
        for outcome in ('success','disabled','failure'):
            expectation=row['expected'] if outcome=='success' else row['unavailable' if outcome=='disabled' else 'failure']
            result[(command,outcome)]=dict(command=command,outcome=outcome,producer_kind='manual',
                setup=row['setup'],steps=[row['action'],expectation,
                    'Retain before/after document and saved-file hashes, exact context, control state, observed result and owned process cleanup.',
                    'Reset the fixture and repeat the complementary path; record FAIL/NOT_RUN honestly. Obtain independent review before importing any result.'],
                expected=expectation,authority=sources,observation_status='NOT_RUN',review='pending',
                complete_case_mapping=False,proposed_inapplicability=expectation.startswith('Proposed inapplicable:'),
                scope='Concrete procedure authoring only; neither an executed result nor an approved exclusion.')
    return result
