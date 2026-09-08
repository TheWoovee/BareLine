// SPDX-License-Identifier: MPL-2.0
//! Native two-pane consumer. Both surfaces address the same document actor when cloned.
use super::*;
use bareline_app::views::{
    Orientation, ScrollPosition, SessionTab, SharedEditorView, ViewController, ViewSnapshot,
    ViewState,
};
use bareline_app::workspace::WorkspaceEditor;
use bareline_commands::{CommandId, CommandPresentation, CommandRegistry, CommandSpec};
use bareline_renderer::{DrawOp, LayoutError, Rect, TextBackend};
use bareline_ui::{ACCENT, BORDER, CHROME, MUTED, TAB_HEIGHT, TEXT, rect, text};
use std::collections::VecDeque;

/// Populate the production tab controller and retained hit geometry for the host
/// accessibility golden. No native window, renderer, dialog or synthetic IDs.
#[cfg(test)]
pub(super) fn accessibility_test_setup(shell: &mut Shell, scenario: &str) {
    shell.views = ViewsRuntime::default();
    shell.workspace = None;
    shell.app.active = 0;
    shell.app.tabs.clear();
    if scenario == "closed" {
        return;
    }
    assert!(
        matches!(
            scenario,
            "open" | "populated" | "focus_close" | "focus_overflow" | "mru" | "vertical"
        ),
        "unknown views accessibility fixture"
    );
    let mut workspace = Workspace::new(
        shell.notify.clone(),
        std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
    )
    .unwrap();
    let count = if scenario == "open" { 2 } else { 14 };
    for _ in 0..count {
        workspace.new_document().unwrap();
    }
    shell.views.sync_documents(&workspace);
    let first = shell.views.controller.as_ref().unwrap().tabs()[0].id;
    {
        let controller = shell.views.controller.as_mut().unwrap();
        controller.pin(first, true).unwrap();
        controller.color(first, Some(0x36c9c6)).unwrap();
        controller.activate(first).unwrap();
    }
    shell.views.install_views(&mut workspace);
    shell.app.tabs = workspace.titles();
    shell.workspace = Some(workspace);
    if scenario == "vertical" {
        assert!(shell.tabs_dispatch("view.tabs.vertical"));
    }
    if scenario == "mru" {
        assert!(shell.tabs_dispatch("view.tabs.mru"));
    }
    let workspace = shell.workspace.as_ref().unwrap();
    let vertical = shell.views.controller.as_ref().unwrap().vertical_tabs;
    let mut operations = Vec::new();
    shell.views.draw_tab_strip(
        workspace,
        0,
        if vertical {
            rect(0.0, 0.0, 176.0, 776.0)
        } else {
            rect(0.0, 0.0, 1000.0, TAB_HEIGHT)
        },
        vertical,
        &mut operations,
    );
    shell
        .views
        .draw_mru(workspace, 1000.0, 800.0, &mut operations);
    assert!(
        !operations.is_empty(),
        "fixture must retain production layout"
    );
    if scenario == "focus_close" {
        let hit = shell
            .views
            .tab_hits
            .iter()
            .find(|hit| hit.id == first)
            .unwrap();
        shell.views.accessibility_focus = access_tab_id(hit.id).map(|id| id + 1);
    } else if scenario == "focus_overflow" {
        let (pane, forward, _) = shell.views.tab_nav.first().unwrap();
        shell.views.accessibility_focus =
            Some(ACCESS_NAV_BASE + *pane as u64 * 2 + u64::from(*forward));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_capture_preserves_unavailable_tab_when_temporary_id_collides() {
        let mut workspace = Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.new_document().unwrap();
        let mut views = ViewsRuntime::default();
        views.sync_documents(&workspace);
        let mut manifest = bareline_file_io::session::SessionManifest {
            documents: vec![
                bareline_file_io::session::SessionDocument {
                    id: 1,
                    path: None,
                    title: "Live".into(),
                },
                bareline_file_io::session::SessionDocument {
                    id: 2,
                    path: None,
                    title: "Awaiting recovery".into(),
                },
            ],
            tabs: vec![
                SessionTab {
                    id: 7,
                    document_id: 1,
                    pinned: false,
                    view: ViewState::default(),
                },
                SessionTab {
                    id: 1,
                    document_id: 2,
                    pinned: false,
                    view: ViewState::default(),
                },
            ],
            active_tab: Some(7),
            ..Default::default()
        };
        views.capture_session(&workspace, &mut manifest, &[(0, 7)]);
        manifest.validate().unwrap();
        assert_eq!(manifest.tabs.len(), 2);
        assert!(manifest.tabs.iter().any(|tab| tab.document_id == 2));
        assert_ne!(manifest.tabs[0].id, manifest.tabs[1].id);
    }

    #[test]
    fn folded_large_gap_keeps_independent_pane_source_layout_and_click() {
        use bareline_document::TextOffset;
        use bareline_editor_surface::paged_view::SourceAffinity;
        let path=std::env::temp_dir().join(format!("bareline-mapped-native-{}.txt",std::process::id()));
        let body=format!("header\n{}suffix\n", "interior row with bounded content\n".repeat(12_000));
        let suffix=body.find("suffix").unwrap(); assert!(suffix>256*1024);
        std::fs::write(&path,&body).unwrap();
        let mut workspace=Workspace::new(std::sync::Arc::new(||{}),std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem)).unwrap();
        workspace.resident_max_bytes=1;workspace.open(path.clone());
        let deadline=Instant::now()+Duration::from_secs(30);
        loop {assert!(Instant::now()<deadline);workspace.pump();if workspace.editors.first().is_some_and(|editor|matches!(editor,WorkspaceEditor::Paged(p) if p.viewport_ready())){break;}std::thread::yield_now();}
        let mut views=ViewsRuntime::default();views.split(&mut workspace,0,Orientation::Vertical);
        loop {assert!(Instant::now()<deadline);workspace.pump();views.pump(&mut workspace);if !views.busy(&workspace)&&views.pending_restore.iter().all(Option::is_none)&&views.pending_view_scroll.iter().all(Option::is_none){break;}std::thread::yield_now();}
        let WorkspaceEditor::Paged(peer)=views.secondary.as_mut().unwrap() else {unreachable!()};
        peer.set_known_global_folds(vec![bareline_syntax::folding::Fold {header:0,end:12_000,level:1}],1,false,0).unwrap();
        loop {assert!(Instant::now()<deadline);peer.pump();if peer.paged_frame_state().ready&&peer.source_segments().len()>1{break;}std::thread::yield_now();}
        assert!(peer.local_offset(TextOffset(100_000)).is_none());
        let suffix_local=peer.local_offset(TextOffset(suffix)).unwrap();
        assert_eq!(peer.source_offset(suffix_local,SourceAffinity::After),Some(TextOffset(suffix)));
        let mut renderer=bareline_renderer_recording::RecordingBackend::default();let mut ops=Vec::new();
        peer.draw_styled(&mut renderer,1000.0,800.0,&mut ops,bareline_editor_surface::SyntaxView{result:None,language:"Plain text",unavailable:false}).unwrap();
        let suffix_box=peer.accessibility_geometry(&renderer,1000.0,800.0).into_iter().find(|(range,_)|range.start==suffix_local.0).unwrap().1;
        let mut boxes=peer.accessibility_geometry(&renderer,1000.0,800.0).into_iter().map(|(range,bounds)|bareline_platform::accessibility::AccessibilityTextBox{start:range.start,end:range.end,bounds:[bounds.x as f64,bounds.y as f64,bounds.width as f64,bounds.height as f64]}).collect();
        super::super::accessibility::map_paged_geometry(peer,&mut boxes);
        let footer=boxes.iter().find(|rect|rect.bounds[0]==suffix_box.x as f64&&rect.bounds[1]==suffix_box.y as f64).unwrap();
        assert_eq!(footer.start,suffix);
        assert!(boxes.iter().any(|rect|rect.start==0));
        assert!(boxes.iter().all(|rect|rect.end<=7||rect.start>=suffix));
        let identity=peer.snapshot().identity_token();
        let source=bareline_app::accessibility::text_source(views.secondary.as_ref().unwrap(),std::sync::Arc::new(||{}));
        assert_eq!(source.len(),body.len());
        assert_eq!(source.identity(),identity);
        loop {
            match source.read(suffix,7) {
                bareline_platform::accessibility::AccessibleRead::Ready{start,text}=>{assert_eq!(start,suffix);assert_eq!(text,"suffix\n");break;},
                bareline_platform::accessibility::AccessibleRead::Pending=>{assert!(Instant::now()<deadline);std::thread::yield_now();},
                _=>panic!("Visible footer unavailable through canonical text reader"),
            }
        }
        let WorkspaceEditor::Paged(peer)=views.secondary.as_mut().unwrap() else {unreachable!()};
        peer.click(&renderer,Point{x:suffix_box.x+0.1,y:suffix_box.y+suffix_box.height*0.5},false).unwrap();
        while peer.busy(){assert!(Instant::now()<deadline);peer.pump();std::thread::yield_now();}
        assert_eq!(peer.global_selection().1.0,suffix);
        let prior=peer.global_selection();peer.request_viewport(TextOffset(0)).unwrap();
        peer.click(&renderer,Point{x:suffix_box.x+0.1,y:suffix_box.y+suffix_box.height*0.5},false).unwrap();
        assert_eq!(peer.global_selection(),prior);
        let WorkspaceEditor::Paged(primary)=&workspace.editors[0] else {unreachable!()};
        assert!(primary.source_segments().is_empty());assert_ne!(primary.global_selection(),prior);
        drop(views);drop(workspace);let _=std::fs::remove_file(path);
    }

    #[test]
    fn paged_split_synchronizes_global_lines_after_pending_lookup() {
        use bareline_editor_surface::paged_view::GlobalScrollPosition;
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let root = std::env::temp_dir().join(format!(
            "bareline-native-global-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("lines.txt");
        std::fs::write(
            &path,
            (0..20_000)
                .map(|line| format!("line {line:05} café\n"))
                .collect::<String>(),
        )
        .unwrap();
        let mut workspace = Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.resident_max_bytes = 1;
        workspace.open(path.clone());
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            assert!(
                Instant::now() < deadline,
                "paged open timed out: {:?}",
                workspace.message
            );
            workspace.pump();
            if workspace.editors.first().is_some_and(|editor| matches!(editor, WorkspaceEditor::Paged(paged) if paged.viewport_ready() && paged.viewport_first_global_line().is_some())) { break; }
            std::thread::yield_now();
        }
        let mut views = ViewsRuntime::default();
        views.split(&mut workspace, 0, Orientation::Vertical);
        loop {
            assert!(Instant::now() < deadline, "paged clone timed out");
            workspace.pump();
            views.pump(&mut workspace);
            if views.pending_restore.iter().all(Option::is_none) && views.pending_view_scroll.iter().all(Option::is_none)
                && views.secondary.as_ref().is_some_and(|editor| matches!(editor, WorkspaceEditor::Paged(paged) if paged.viewport_ready() && paged.viewport_first_global_line().is_some())) { break; }
            std::thread::yield_now();
        }
        views.controller.as_mut().unwrap().sync_vertical = true;
        views.controller.as_mut().unwrap().sync_horizontal = true;
        views.secondary.as_mut().unwrap().zoom_by(4.0);
        views.set_compare_alignment(Some(
            bareline_app::views::AlignmentMap::new(vec![bareline_app::views::AlignmentBlock {
                left: 7000..7000,
                right: 7000..7003,
            }])
            .unwrap(),
        ));
        let WorkspaceEditor::Paged(source) = &mut workspace.editors[0] else {
            panic!("expected paged source")
        };
        source.request_global_scroll(8000, 0.25, 37.0).unwrap();
        views.sync_scroll(&mut workspace, 0);
        assert!(
            views.pending_sync.is_some(),
            "an in-flight source navigation must not publish its old line"
        );
        loop {
            assert!(
                Instant::now() < deadline,
                "global synchronized navigation timed out"
            );
            workspace.pump();
            views.pump(&mut workspace);
            let Some(WorkspaceEditor::Paged(target)) = &mut views.secondary else {
                panic!("expected paged clone")
            };
            if matches!(target.global_logical_scroll(), GlobalScrollPosition::Ready(8003, fraction, x) if (fraction - 0.25).abs() < 0.00001 && x == 37.0)
            {
                break;
            }
            std::thread::yield_now();
        }
        assert!(views.pending_sync.is_none());
        assert!(views.queued.is_empty());
        let Some(WorkspaceEditor::Paged(target)) = &mut views.secondary else {
            unreachable!()
        };
        let original_viewport = target.viewport_start();
        let selection_token = target.restore_global_selection(bareline_document::TextOffset(2), bareline_document::TextOffset(9), true).unwrap();
        loop {
            target.pump();
            match target.selection_restore_status(selection_token) {
                bareline_editor_surface::paged_view::SelectionRestoreStatus::Pending => { assert!(Instant::now() < deadline); std::thread::yield_now(); }
                bareline_editor_surface::paged_view::SelectionRestoreStatus::Applied => break,
                status => panic!("Global selection failed: {status:?}"),
            }
        }
        assert_eq!(target.viewport_start(), original_viewport);
        assert_eq!(target.global_selection(), (bareline_document::TextOffset(2), bareline_document::TextOffset(9)));
        let mut peer = target.clone_view().unwrap();
        peer.pump();
        assert_eq!(peer.global_selection(), target.global_selection());
        target.surface.scroll_y = 17.5;
        target.restore_global_folds(&[8004..8011]);
        let saved_byte = target.viewport_start();
        target.request_viewport(saved_byte).unwrap();
        assert_eq!(
            target.global_logical_scroll(),
            GlobalScrollPosition::Pending
        );
        let mut pending_ops = Vec::new();
        assert!(bareline_app::workspace::paint_paged_pending(views.secondary.as_ref().unwrap(),1000.0,800.0,workspace.theme,&mut pending_ops));
        assert!(!pending_ops.iter().any(|op|matches!(op,DrawOp::Layout {..})));
        let expected = workspace_view_state(views.secondary.as_ref().unwrap());
        assert_eq!(expected.scroll_byte, Some(saved_byte.0 as u64));
        assert_eq!(expected.folds, vec![8004..8011]);
        let mut manifest = bareline_file_io::session::SessionManifest {
            documents: vec![bareline_file_io::session::SessionDocument {
                id: 1,
                path: None,
                title: "Paged".into(),
            }],
            tabs: vec![SessionTab {
                id: 1,
                document_id: 1,
                pinned: false,
                view: ViewState::default(),
            }],
            active_tab: Some(1),
            ..Default::default()
        };
        views.capture_session(&workspace, &mut manifest, &[(0, 1)]);
        let manifest = bareline_file_io::session::decode(
            &bareline_file_io::session::encode(&manifest).unwrap(),
        )
        .unwrap();
        let mut restored = ViewsRuntime::default();
        let mut app = App::default();
        restored.restore_session(&mut workspace, &mut app, &manifest, &[(1, 0), (2, 0)]);
        loop {
            assert!(
                Instant::now() < deadline,
                "byte anchored view restoration timed out"
            );
            workspace.pump();
            restored.pump(&mut workspace);
            if restored.pending_restore.iter().all(Option::is_none)
                && restored.pending_view_scroll.iter().all(Option::is_none)
                && !restored.busy(&workspace)
            {
                break;
            }
            std::thread::yield_now();
        }
        let actual = workspace_view_state(restored.secondary.as_ref().unwrap());
        assert_eq!(
            (
                actual.scroll_byte,
                actual.anchor,
                actual.caret,
                actual.scroll_y_bits,
                actual.scroll_x,
                actual.folds
            ),
            (
                expected.scroll_byte,
                expected.anchor,
                expected.caret,
                expected.scroll_y_bits,
                expected.scroll_x,
                expected.folds
            )
        );
        let editor = restored.secondary.as_mut().unwrap();
        let before = editor.selection;
        let mut invalid = workspace_view_state(editor);
        let WorkspaceEditor::Paged(paged) = &mut *editor else {
            unreachable!()
        };
        let text = paged
            .surface
            .snapshot()
            .read(
                bareline_document::TextOffset(0)
                    ..bareline_document::TextOffset(paged.surface.snapshot().len()),
                64 * 1024,
            )
            .unwrap();
        invalid.anchor = paged.source_offset(bareline_document::TextOffset(text.find('é').unwrap() + 1), bareline_editor_surface::paged_view::SourceAffinity::After).unwrap().0 as u64;
        let mut token = None;
        loop {
            match finish_workspace_view_restore(editor, &invalid, &mut token) {
                Err(_) => break,
                Ok(false) => {
                    while editor.busy() { assert!(Instant::now() < deadline); editor.pump(); std::thread::yield_now(); }
                }
                Ok(true) => panic!("Invalid UTF-8 endpoint was accepted"),
            }
        }
        let WorkspaceEditor::Paged(paged) = &*editor else { unreachable!() };
        assert!(matches!(paged.selection_restore_status(token.unwrap()), bareline_editor_surface::paged_view::SelectionRestoreStatus::Failed(_)));
        assert_eq!(paged.global_selection(), (bareline_document::TextOffset(2), bareline_document::TextOffset(9)));
        assert_eq!(editor.selection, before);
        let mut invalid = workspace_view_state(editor);
        invalid.scroll_byte = Some(u64::MAX);
        assert!(restore_workspace_view(editor, &invalid).is_err());
        assert_eq!(editor.selection, before);
        let guarded = PendingViewScroll {
            selection_token: None,
            state: workspace_view_state(&workspace.editors[0]),
            document: DocumentBinding::new(0, &workspace.editors[0]),
        };
        workspace.editors[0].enqueue(Input::Insert("changed".into()));
        while workspace.editors[0].busy() {
            assert!(Instant::now() < deadline);
            workspace.pump();
            std::thread::yield_now();
        }
        let selection = workspace.editors[0].selection;
        restored.pending_view_scroll[0] = Some(guarded);
        restored.pump(&mut workspace);
        assert_eq!(workspace.editors[0].selection, selection);
        assert!(
            workspace.editors[0]
                .error
                .as_deref()
                .is_some_and(|error| error.contains("changed while"))
        );
        drop(restored);
        drop(views);
        drop(workspace);
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir(root);
    }

    #[test]
    fn delayed_restore_guard_rejects_a_different_document_with_identical_text() {
        let mut workspace = Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.new_document().unwrap();
        workspace.new_document().unwrap();
        let guard = DocumentBinding::new(0, &workspace.editors[0]);
        assert!(guard.matches_state(&workspace.editors[0]));
        assert!(!guard.matches_state(&workspace.editors[1]));
    }

    #[test]
    fn completion_target_tracks_secondary_cursor_and_rejects_focus_change() {
        let mut workspace = Workspace::new(std::sync::Arc::new(|| {}), std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem)).unwrap();
        workspace.new_document().unwrap();
        workspace.editors[0].enqueue(Input::Insert("alpha".into()));
        let deadline = Instant::now() + Duration::from_secs(5);
        while workspace.editors[0].busy() { workspace.pump(); assert!(Instant::now() < deadline); std::thread::yield_now(); }
        let mut views = ViewsRuntime::default(); views.split(&mut workspace, 0, Orientation::Vertical);
        views.activate(&workspace, &mut App::default(), 1);
        assert_eq!(bareline_app::accessibility::source_identity(views.secondary.as_ref().unwrap()), bareline_app::accessibility::source_identity(&workspace.editors[0]));
        views.secondary.as_mut().unwrap().enqueue(Input::SetCaret(1, false));
        while views.secondary.as_ref().unwrap().busy() { views.secondary.as_mut().unwrap().pump(); assert!(Instant::now() < deadline); std::thread::yield_now(); }
        let requested = super::super::language::completion_target(&views, &workspace, 0).unwrap();
        workspace.editors[0].enqueue(Input::SetCaret(2, false));
        while workspace.editors[0].busy() { workspace.pump(); assert!(Instant::now() < deadline); std::thread::yield_now(); }
        assert_eq!(super::super::language::completion_target(&views, &workspace, 0), Some(requested.clone()), "inactive primary cursor must not redirect a secondary request");
        views.secondary.as_mut().unwrap().enqueue(Input::SetCaret(3, false));
        while views.secondary.as_ref().unwrap().busy() { views.secondary.as_mut().unwrap().pump(); assert!(Instant::now() < deadline); std::thread::yield_now(); }
        assert_ne!(super::super::language::completion_target(&views, &workspace, 0), Some(requested.clone()), "selection movement makes completion stale");
        views.secondary.as_mut().unwrap().enqueue(Input::SetCaret(1, false));
        while views.secondary.as_ref().unwrap().busy() { views.secondary.as_mut().unwrap().pump(); assert!(Instant::now() < deadline); std::thread::yield_now(); }
        assert_eq!(super::super::language::completion_target(&views, &workspace, 0), Some(requested.clone()));
        views.activate(&workspace, &mut App::default(), 0);
        assert_ne!(super::super::language::completion_target(&views, &workspace, 0), Some(requested), "pane identity must prevent accepting into another clone");
    }
    #[test]
    fn delayed_fold_result_targets_the_requested_view_after_focus_moves() {
        let mut workspace = Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.new_document().unwrap();
        workspace.editors[0].enqueue(Input::Insert("a\nb\nc\n".into()));
        let deadline = Instant::now() + Duration::from_secs(5);
        while workspace.editors[0].busy() {
            assert!(Instant::now() < deadline);
            workspace.pump();
            std::thread::yield_now();
        }
        let mut views = ViewsRuntime::default();
        views.split(&mut workspace, 0, Orientation::Vertical);
        views.record_fold_target();
        let source = workspace.editors[0].snapshot().clone();
        views.activate(&workspace, &mut App::default(), 0);
        views.apply_fold_result(
            &mut workspace,
            &source,
            vec![bareline_syntax::folding::Fold {
                header: 0,
                end: 2,
                level: 1,
            }],
            1,
            false,
        );
        views.pump(&mut workspace);
        assert!(workspace.editors[0].persisted_folds().is_empty());
        assert_eq!(
            views.secondary.as_ref().unwrap().persisted_folds(),
            vec![0..3]
        );
        views.record_fold_target();
        workspace.editors[0].enqueue(Input::Insert("changed".into()));
        while workspace.editors[0].busy() {
            assert!(Instant::now() < deadline);
            workspace.pump();
            std::thread::yield_now();
        }
        views.apply_fold_result(
            &mut workspace,
            &source,
            vec![bareline_syntax::folding::Fold {
                header: 0,
                end: 2,
                level: 1,
            }],
            1,
            false,
        );
        assert!(workspace.editors[0].persisted_folds().is_empty());
    }

    #[test]
    fn promotion_rebinds_linked_views_without_replacing_tabs_or_history() {
        let mut workspace=Workspace::new(std::sync::Arc::new(||{}),std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem)).unwrap();
        workspace.new_document().unwrap();
        workspace.editors[0].enqueue(Input::Insert("alpha\nbeta\n".into()));
        let deadline=Instant::now()+Duration::from_secs(30);
        while workspace.editors[0].busy(){assert!(Instant::now()<deadline);workspace.pump();std::thread::yield_now();}
        let mut views=ViewsRuntime::default(); views.split(&mut workspace,0,Orientation::Vertical);
        workspace.editors[0].enqueue(Input::SetCaret(2,false));
        views.secondary.as_mut().unwrap().enqueue(Input::SetCaret(8,false));
        while views.busy(&workspace){assert!(Instant::now()<deadline);workspace.pump();views.pump(&mut workspace);std::thread::yield_now();}
        assert_eq!(workspace.editors[0].selection.caret,2);
        assert_eq!(views.secondary.as_ref().unwrap().selection.caret,8);
        let ids=views.loaded_tabs;
        let identity=workspace.editors[0].snapshot().identity_token();
        assert!(!workspace.promote_resident_for_source_edit(0,identity).unwrap());
        loop {
            assert!(Instant::now()<deadline, "promotion convergence: message={:?}; primary paged={} busy={} error={:?}; secondary paged={} busy={} error={:?}; restore={:?}; scroll={:?}", workspace.message,workspace.editors[0].paged(),workspace.editors[0].busy(),workspace.editors[0].error,views.secondary.as_ref().is_some_and(WorkspaceEditor::paged),views.secondary.as_ref().is_some_and(WorkspaceEditor::busy),views.secondary.as_ref().and_then(|e|e.error.as_ref()),views.pending_restore.iter().map(Option::is_some).collect::<Vec<_>>(),views.pending_view_scroll.iter().map(Option::is_some).collect::<Vec<_>>());
            workspace.pump();views.pump(&mut workspace);
            workspace.promote_resident_for_source_edit(0,identity).unwrap_or_else(|error|panic!("promotion failed before view rebind: {error}"));
            if workspace.editors[0].paged() && views.secondary.as_ref().is_some_and(WorkspaceEditor::paged) && !views.busy(&workspace) && views.pending_restore.iter().all(Option::is_none) && views.pending_view_scroll.iter().all(Option::is_none){break;}
            std::thread::yield_now();
        }
        assert_eq!(views.loaded_tabs,ids);
        let WorkspaceEditor::Paged(primary)=&workspace.editors[0] else {unreachable!()};
        let WorkspaceEditor::Paged(peer)=views.secondary.as_ref().unwrap() else {unreachable!()};
        assert!(primary.snapshot().same_document(peer.snapshot()));
        assert_eq!(primary.global_selection().1.0,2,"promotion selection error: {:?}, message: {:?}",primary.error,workspace.message);
        assert_eq!(peer.global_selection().1.0,8);
        assert!(primary.can_undo());
        views.secondary.as_mut().unwrap().enqueue(Input::Insert("X".into()));
        loop {
            assert!(Instant::now()<deadline);workspace.pump();views.pump(&mut workspace);
            if !views.busy(&workspace){break;} std::thread::yield_now();
        }
        let WorkspaceEditor::Paged(primary)=&workspace.editors[0] else {unreachable!()};
        let WorkspaceEditor::Paged(peer)=views.secondary.as_ref().unwrap() else {unreachable!()};
        assert_eq!(primary.snapshot().revision,peer.snapshot().revision);
        assert_eq!(primary.snapshot().len(),12);
        workspace.editors[0].enqueue(Input::Undo);
        loop {assert!(Instant::now()<deadline);workspace.pump();views.pump(&mut workspace);if !views.busy(&workspace){break;}std::thread::yield_now();}
        let WorkspaceEditor::Paged(primary)=&workspace.editors[0] else {unreachable!()};
        assert_eq!(primary.snapshot().len(),11);
    }

    #[test]
    fn queued_edits_in_both_native_panes_share_the_document() {
        let mut workspace = Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.new_document().unwrap();
        let mut views = ViewsRuntime::default();
        views.split(&mut workspace, 0, Orientation::Vertical);
        views.input(&mut workspace, 0, Input::Insert("first".into()));
        views.input(&mut workspace, 1, Input::End(false));
        views.input(&mut workspace, 1, Input::Insert(" second".into()));
        let deadline = Instant::now() + Duration::from_secs(5);
        while views.busy(&workspace) {
            assert!(
                Instant::now() < deadline,
                "pane edit acknowledgment timed out"
            );
            workspace.pump();
            views.pump(&mut workspace);
            std::thread::yield_now();
        }
        let primary = workspace.editors[0].snapshot().clone();
        let secondary = views.secondary.as_ref().unwrap().snapshot();
        assert!(primary.same_document(secondary));
        assert_eq!(primary.revision, secondary.revision);
        assert_eq!(
            primary
                .read(
                    bareline_document::TextOffset(0)..bareline_document::TextOffset(primary.len()),
                    100
                )
                .unwrap(),
            "first second"
        );
        workspace.editors[0].set_font_family("Consolas").unwrap();
        views.pump(&mut workspace);
        assert_eq!(views.secondary.as_ref().unwrap().font_family(), "Consolas");
        workspace.editors[0].set_logical_scroll(0, 0.0, 84.0);
        views
            .secondary
            .as_mut()
            .unwrap()
            .set_logical_scroll(0, 0.0, 13.0);
        views.controller.as_mut().unwrap().sync_vertical = true;
        views.sync_scroll(&mut workspace, 0);
        assert_eq!(views.secondary.as_ref().unwrap().logical_scroll().2, 13.0);
        views.controller.as_mut().unwrap().sync_horizontal = true;
        views.sync_scroll(&mut workspace, 0);
        assert_eq!(views.secondary.as_ref().unwrap().logical_scroll().2, 84.0);
        views.secondary.as_mut().unwrap().selection.anchor = 6;
        views.secondary.as_mut().unwrap().scroll_y = 40.0;
        views.controller.as_mut().unwrap().ratio = 0.65;
        let mut manifest = bareline_file_io::session::SessionManifest::default();
        manifest
            .documents
            .push(bareline_file_io::session::SessionDocument {
                id: 1,
                path: None,
                title: "Untitled".into(),
            });
        manifest.tabs.push(SessionTab {
            id: 1,
            document_id: 1,
            pinned: false,
            view: ViewState::default(),
        });
        manifest.active_tab = Some(1);
        views.capture_session(&workspace, &mut manifest, &[(0, 1)]);
        manifest.validate().unwrap();
        assert_eq!(manifest.tabs.len(), 2);
        let mut restored = ViewsRuntime::default();
        let mut app = App::default();
        restored.restore_session(&mut workspace, &mut app, &manifest, &[(1, 0), (2, 0)]);
        assert!(restored.open());
        assert_eq!(restored.secondary.as_ref().unwrap().selection.anchor, 6);
        assert_eq!(restored.secondary.as_ref().unwrap().scroll_y, 40.0);
        assert_eq!(restored.controller.as_ref().unwrap().ratio, 0.65);
        views.collapse(&mut workspace, true);
        assert!(!views.open());
        assert_eq!(workspace.editors[0].snapshot().revision, primary.revision);
    }
}

pub(super) fn register(registry: &mut CommandRegistry) {
    for (id, title, shortcut) in [
        ("view.split_vertical", "Split Vertically", ""),
        ("view.split_horizontal", "Split Horizontally", ""),
        ("view.clone_other", "Clone to Other View", ""),
        ("view.move_other", "Move to Other View", ""),
        ("view.close_split", "Close Split View", ""),
        ("view.focus_other", "Focus Other View", "F6"),
        ("view.sync_vertical", "Synchronize Vertical Scrolling", ""),
        (
            "view.sync_horizontal",
            "Synchronize Horizontal Scrolling",
            "",
        ),
        ("view.tabs.vertical", "Vertical Tabs", ""),
        ("view.tabs.pin", "Pin or Unpin Tab", ""),
        ("view.tabs.color", "Cycle Tab Color", ""),
        ("view.tabs.sort_name", "Sort Tabs by Name", ""),
        ("view.tabs.sort_path", "Sort Tabs by Path", ""),
        ("view.tabs.sort_descending", "Sort Tabs Descending", ""),
        ("view.tabs.move_left", "Move Tab Left", "Ctrl+Shift+PageUp"),
        (
            "view.tabs.move_right",
            "Move Tab Right",
            "Ctrl+Shift+PageDown",
        ),
        ("view.tabs.previous", "Previous Tab", "Ctrl+PageUp"),
        ("view.tabs.next", "Next Tab", "Ctrl+PageDown"),
        ("view.tabs.mru", "Recent Document Switcher", "Ctrl+Tab"),
    ] {
        let id = CommandId(id);
        let _ = registry.register(CommandSpec {
            id,
            title,
            category: "View",
            shortcut,
            action: Action::Contributed(id),
        });
        let _ = registry.set_presentation(
            id,
            CommandPresentation {
                menu_path: format!("View > {title}"),
                keywords: vec!["split".into(), "pane".into()],
                accessible_name: Some(title.into()),
                ..Default::default()
            },
        );
    }
}

struct QueuedInput {
    pane: u32,
    document: DocumentBinding,
    input: Input,
}
#[derive(Clone)]
enum DocumentBinding {
    Resident(u64, ViewSnapshot),
    Paged(u64, bareline_document::paged::PagedSnapshot),
}
impl DocumentBinding {
    fn id(&self) -> u64 {
        match self {
            Self::Resident(id, _) | Self::Paged(id, _) => *id,
        }
    }
    fn new(id: u64, editor: &bareline_app::workspace::WorkspaceEditor) -> Self {
        match editor {
            bareline_app::workspace::WorkspaceEditor::Resident(editor) => {
                Self::Resident(id, editor.snapshot().clone())
            }
            bareline_app::workspace::WorkspaceEditor::Paged(editor) => {
                Self::Paged(id, editor.snapshot().clone())
            }
        }
    }
    fn matches_state(&self, editor: &WorkspaceEditor) -> bool {
        self.matches(editor)
            && match (self, editor) {
                (Self::Resident(_, snapshot), WorkspaceEditor::Resident(editor)) => {
                    snapshot.content_state == editor.snapshot().content_state
                }
                (Self::Paged(_, snapshot), WorkspaceEditor::Paged(editor)) => {
                    snapshot.content_state == editor.snapshot().content_state
                }
                _ => false,
            }
    }
    fn matches(&self, editor: &bareline_app::workspace::WorkspaceEditor) -> bool {
        match (self, editor) {
            (
                Self::Resident(_, snapshot),
                bareline_app::workspace::WorkspaceEditor::Resident(editor),
            ) => snapshot.same_document(editor.snapshot()),
            (Self::Paged(_, snapshot), bareline_app::workspace::WorkspaceEditor::Paged(editor)) => {
                snapshot.same_document(editor.snapshot())
            }
            (Self::Resident(_, snapshot), WorkspaceEditor::Paged(editor)) => snapshot.identity_token().0 == editor.snapshot().identity_token().0,
            _ => false,
        }
    }
}
#[derive(Clone, Copy)]
struct TabHit {
    id: u64,
    pane: u32,
    bounds: Rect,
    close: Rect,
}
struct TabDrag {
    id: u64,
    start: Point,
    moved: bool,
}
struct MruPopup {
    ids: Vec<u64>,
    selected: usize,
    bounds: Rect,
}
struct PendingViewScroll {
    selection_token: Option<u64>,
    state: ViewState,
    document: DocumentBinding,
}
#[derive(Default)]
pub(super) struct ViewsRuntime {
    documents: Vec<DocumentBinding>,
    closed_documents: VecDeque<(DocumentBinding, Vec<(usize, SessionTab, Option<u32>)>)>,
    next_document: u64,
    loaded_tabs: [Option<u64>; 2],
    pending_restore: [Option<ViewState>; 2],
    pending_view_scroll: [Option<PendingViewScroll>; 2],
    styling: [bareline_app::ViewStyling; 2],
    tab_hits: Vec<TabHit>,
    tab_strips: [Option<Rect>; 2],
    tab_nav: Vec<(u32, bool, Rect)>,
    tab_offset: [usize; 2],
    tab_drag: Option<TabDrag>,
    mru_popup: Option<MruPopup>,
    accessibility_focus: Option<u64>,
    pending_close: Option<usize>,
    controller: Option<ViewController>,
    primary: Option<ViewSnapshot>,
    pub(super) secondary: Option<WorkspaceEditor>,
    retired: Vec<WorkspaceEditor>,
    pub(super) bounds: [Option<Rect>; 2],
    splitter: Option<Rect>,
    dragging: bool,
    queued: VecDeque<QueuedInput>,
    compare: bool,
    alignment: Option<bareline_app::views::AlignmentMap>,
    applied_spacers: [Option<Vec<(u64, u64)>>; 2],
    pending_sync: Option<(u32, u64)>,
    fold_target: Option<(u32, u64)>,
}
impl ViewsRuntime {
    fn draw_tab_strip(
        &mut self,
        workspace: &Workspace,
        pane: u32,
        bounds: Rect,
        vertical: bool,
        ops: &mut Vec<DrawOp>,
    ) {
        self.tab_strips[pane as usize] = Some(bounds);
        let Some(controller) = &self.controller else {
            return;
        };
        let tabs: Vec<_> = controller.pane_tabs(pane).cloned().collect();
        let step = if vertical {
            TAB_HEIGHT
        } else {
            bareline_ui::controls::TabStrip::TAB_WIDTH
        };
        let extent = if vertical {
            bounds.height
        } else {
            bounds.width
        };
        let count = ((extent - 48.0) / step).floor().max(1.0) as usize;
        let start = self.tab_offset[pane as usize].min(tabs.len().saturating_sub(count));
        self.tab_offset[pane as usize] = start;
        ops.push(DrawOp::Fill(bounds, workspace.theme.chrome));
        ops.push(DrawOp::PushClip(bounds));
        let titles = workspace.titles();
        for (row, tab) in tabs.iter().skip(start).take(count).enumerate() {
            let bounds = if vertical {
                rect(
                    bounds.x,
                    bounds.y + row as f32 * step,
                    bounds.width,
                    TAB_HEIGHT,
                )
            } else {
                rect(bounds.x + row as f32 * step, bounds.y, step, TAB_HEIGHT)
            };
            let selected = controller.active_tab(pane) == Some(tab.id);
            let index = self.document_index(workspace, tab.document_id);
            let title = index
                .and_then(|index| titles.get(index))
                .map(String::as_str)
                .unwrap_or("Document");
            let dirty = index.is_some_and(|index| workspace.editors[index].dirty());
            ops.push(DrawOp::Fill(
                bounds,
                if selected {
                    workspace.theme.editor
                } else {
                    workspace.theme.chrome
                },
            ));
            ops.push(DrawOp::Stroke(bounds, workspace.theme.border, 1.0));
            if let Some(color) = controller.tab_colors.get(&tab.id) {
                ops.push(DrawOp::Fill(
                    rect(bounds.x, bounds.y, 4.0, bounds.height),
                    bareline_renderer::Color(*color),
                ));
            }
            let label = format!(
                "{}{}{}",
                if tab.pinned { "◆ " } else { "" },
                title.chars().take(18).collect::<String>(),
                if dirty { " •" } else { "" }
            );
            text(
                ops,
                bounds.x + 10.0,
                bounds.y + 8.0,
                label,
                13.0,
                if selected {
                    workspace.theme.text
                } else {
                    workspace.theme.muted
                },
            );
            let close = rect(
                bounds.x + bounds.width - 24.0,
                bounds.y,
                24.0,
                bounds.height,
            );
            text(
                ops,
                close.x + 6.0,
                close.y + 7.0,
                "×",
                14.0,
                workspace.theme.muted,
            );
            if selected {
                ops.push(DrawOp::Fill(
                    rect(bounds.x, bounds.y + bounds.height - 2.0, bounds.width, 2.0),
                    workspace.theme.focus,
                ));
            }
            self.tab_hits.push(TabHit {
                id: tab.id,
                pane,
                bounds,
                close,
            });
        }
        for (next, offset, label) in [(false, 48.0, "‹"), (true, 24.0, "›")] {
            let nav = if vertical {
                rect(
                    bounds.x + if next { bounds.width / 2.0 } else { 0.0 },
                    bounds.y + bounds.height - 24.0,
                    bounds.width / 2.0,
                    24.0,
                )
            } else {
                rect(bounds.x + bounds.width - offset, bounds.y, 24.0, TAB_HEIGHT)
            };
            text(
                ops,
                nav.x + 8.0,
                nav.y + 6.0,
                label,
                14.0,
                workspace.theme.text,
            );
            self.tab_nav.push((pane, next, nav));
        }
        ops.push(DrawOp::PopClip);
    }
    fn draw_mru(&mut self, workspace: &Workspace, width: f32, height: f32, ops: &mut Vec<DrawOp>) {
        let Some(popup) = &self.mru_popup else {
            return;
        };
        let ids = popup.ids.clone();
        let selected = popup.selected;
        let bounds = rect(
            (width - 360.0).max(0.0) / 2.0,
            TAB_HEIGHT + 12.0,
            width.min(360.0),
            (height - 80.0).clamp(0.0, 12.0 * TAB_HEIGHT),
        );
        self.mru_popup.as_mut().unwrap().bounds = bounds;
        ops.push(DrawOp::Fill(bounds, workspace.theme.chrome));
        ops.push(DrawOp::Stroke(bounds, workspace.theme.border, 1.0));
        ops.push(DrawOp::PushClip(bounds));
        let titles = workspace.titles();
        let visible = mru_visible_rows(bounds);
        let start = selected.saturating_sub(visible.saturating_sub(1));
        for (row, id) in ids.iter().skip(start).take(visible).enumerate() {
            let row_bounds = rect(
                bounds.x,
                bounds.y + row as f32 * TAB_HEIGHT,
                bounds.width,
                TAB_HEIGHT,
            );
            if row + start == selected {
                ops.push(DrawOp::Fill(row_bounds, workspace.theme.interactive));
            }
            let title = self
                .tab_index(workspace, *id)
                .and_then(|index| titles.get(index))
                .cloned()
                .unwrap_or_default();
            text(
                ops,
                row_bounds.x + 12.0,
                row_bounds.y + 8.0,
                title,
                13.0,
                workspace.theme.text,
            );
        }
        ops.push(DrawOp::PopClip);
    }
    fn document_index(&self, workspace: &Workspace, id: u64) -> Option<usize> {
        let binding = self.documents.iter().find(|binding| binding.id() == id)?;
        workspace
            .editors
            .iter()
            .position(|editor| binding.matches(editor))
    }
    fn tab_index(&self, workspace: &Workspace, id: u64) -> Option<usize> {
        self.document_index(workspace, self.controller.as_ref()?.tab(id)?.document_id)
    }
    fn sync_documents(&mut self, workspace: &Workspace) {
        // Promotion preserves logical identity but changes the actor facade. Rebind
        // existing linked panes even though their stable tab IDs did not change.
        for binding in &mut self.documents {
            let DocumentBinding::Resident(id, source) = binding else { continue; };
            let Some(WorkspaceEditor::Paged(promoted)) = workspace.editors.iter().find(|editor| matches!(editor,WorkspaceEditor::Paged(paged) if paged.snapshot().identity_token().0 == source.identity_token().0)) else { continue; };
            if self.secondary.as_ref().is_some_and(|peer| matches!(peer,WorkspaceEditor::Resident(resident) if resident.snapshot().same_document(source))) {
                let old=self.secondary.as_ref().unwrap();
                let state=workspace_view_state(old);
                match promoted.clone_view() {
                    Ok(mut peer) => {
                        old.copy_presentation_to(&mut peer.surface);
                        let old=self.secondary.replace(WorkspaceEditor::Paged(peer)).unwrap();
                        self.retired.push(old);
                        self.pending_restore[1]=Some(state);
                        self.pending_view_scroll[1]=None;
                        self.applied_spacers[1]=None;
                    }
                    Err(_) => continue, // Retry without discarding the linked view.
                }
            }
            if self.primary.as_ref().is_some_and(|primary|primary.same_document(source)) {
                self.primary=Some(promoted.surface.snapshot().clone());
                self.applied_spacers[0]=None;
            }
            *binding=DocumentBinding::Paged(*id,promoted.snapshot().clone());
        }
        if self.controller.is_some()
            && self.documents.len() == workspace.editors.len()
            && self
                .documents
                .iter()
                .zip(&workspace.editors)
                .all(|(binding, editor)| binding.matches(editor))
        {
            return;
        }
        if self.controller.is_none() {
            self.controller = ViewController::new(Vec::new(), None).ok();
        }
        if let Some(controller) = &self.controller {
            for binding in &self.documents {
                if !workspace
                    .editors
                    .iter()
                    .any(|editor| binding.matches(editor))
                {
                    let tabs = controller
                        .tabs()
                        .iter()
                        .enumerate()
                        .filter(|(_, tab)| tab.document_id == binding.id())
                        .map(|(position, tab)| {
                            (
                                position,
                                tab.clone(),
                                controller.tab_colors.get(&tab.id).copied(),
                            )
                        })
                        .collect();
                    self.closed_documents.push_back((binding.clone(), tabs));
                    while self.closed_documents.len() > 20 {
                        self.closed_documents.pop_front();
                    }
                }
            }
        }
        self.documents.retain(|binding| {
            workspace
                .editors
                .iter()
                .any(|editor| binding.matches(editor))
        });
        for editor in &workspace.editors {
            if !self.documents.iter().any(|binding| binding.matches(editor)) {
                if let Some(position) = self
                    .closed_documents
                    .iter()
                    .position(|(binding, _)| binding.matches(editor))
                {
                    let (binding, tabs) = self.closed_documents.remove(position).unwrap();
                    self.documents.push(binding);
                    if let Some(controller) = &mut self.controller {
                        for (position, tab, color) in tabs {
                            let _ = controller.restore_tab(tab, position, color);
                        }
                    }
                    continue;
                }
                self.next_document = self.next_document.saturating_add(1);
                self.documents
                    .push(DocumentBinding::new(self.next_document, editor));
                if let Some(controller) = &mut self.controller {
                    if let Ok(id) = controller.add_document(self.next_document) {
                        let _ = controller.set_view_state(id, workspace_view_state(editor));
                    }
                }
            }
        }
        let live: Vec<_> = self.documents.iter().map(DocumentBinding::id).collect();
        if let Some(controller) = &mut self.controller {
            controller.retain_documents(&live);
        }
        self.documents.sort_by_key(|binding| {
            workspace
                .editors
                .iter()
                .position(|editor| binding.matches(editor))
                .unwrap_or(usize::MAX)
        });
    }
    fn save_view_states(&self, workspace: &Workspace, controller: &mut ViewController) {
        for pane in 0..2 {
            let Some(id) = self.loaded_tabs[pane] else {
                continue;
            };
            let editor = if pane == 1 {
                self.secondary.as_ref()
            } else {
                self.primary_index(workspace)
                    .map(|index| &workspace.editors[index])
            };
            if let Some(editor) = editor {
                let mut state = self.pending_restore[pane]
                    .as_ref()
                    .or(self.pending_view_scroll[pane]
                        .as_ref()
                        .map(|pending| &pending.state))
                    .cloned()
                    .unwrap_or_else(|| workspace_view_state(editor));
                if self.pending_restore[pane].is_none()
                    && self.pending_view_scroll[pane].is_none()
                    && matches!(editor, WorkspaceEditor::Paged(paged) if paged.viewport_first_global_line().is_none())
                {
                    if let Some(previous) = controller.tab(id) {
                        state.scroll_line = previous.view.scroll_line;
                    }
                }
                let _ = controller.set_view_state(id, state);
            }
        }
    }
    fn save_current(&mut self, workspace: &Workspace) {
        if let Some(mut controller) = self.controller.clone() {
            self.save_view_states(workspace, &mut controller);
            self.controller = Some(controller);
        }
    }
    fn install_views(&mut self, workspace: &mut Workspace) {
        let Some(controller) = &self.controller else {
            return;
        };
        let ids = [
            controller.active_tab(0),
            if controller.split {
                controller.active_tab(1)
            } else {
                None
            },
        ];
        for pane in 0..2 {
            if ids[pane] == self.loaded_tabs[pane] {
                continue;
            }
            self.applied_spacers[pane] = None;
            let tab = ids[pane]
                .and_then(|id| self.controller.as_ref().unwrap().tab(id))
                .cloned();
            let index = tab
                .as_ref()
                .and_then(|tab| self.document_index(workspace, tab.document_id));
            if pane == 0 {
                self.pending_restore[pane] = None;
                self.pending_view_scroll[pane] = None;
                if let (Some(index), Some(tab)) = (index, tab) {
                    self.primary = Some(workspace.editors[index].snapshot().clone());
                    if workspace.editors[index].paged() {
                        self.pending_restore[pane] = Some(tab.view);
                    } else {
                        if let Err(error) =
                            restore_workspace_view(&mut workspace.editors[index], &tab.view)
                        {
                            workspace.editors[index].error = Some(error);
                        }
                    }
                } else {
                    self.primary = None;
                }
            } else {
                self.pending_restore[pane] = None;
                self.pending_view_scroll[pane] = None;
                if let Some(old) = self.secondary.take() {
                    self.retired.push(old);
                }
                if let (Some(index), Some(tab)) = (index, tab) {
                    let peer = match &workspace.editors[index] {
                        WorkspaceEditor::Resident(editor) => {
                            Ok(WorkspaceEditor::Resident(editor.clone_view()))
                        }
                        WorkspaceEditor::Paged(editor) => {
                            editor.clone_view().map(WorkspaceEditor::Paged)
                        }
                    };
                    match peer {
                        Ok(mut peer) => {
                            if peer.paged() {
                                self.pending_restore[pane] = Some(tab.view);
                            } else {
                                if let Err(error) = restore_workspace_view(&mut peer, &tab.view) {
                                    peer.error = Some(error);
                                }
                            }
                            self.secondary = Some(peer);
                        }
                        Err(error) => workspace.message = Some(error),
                    }
                }
            }
            self.loaded_tabs[pane] = ids[pane];
        }
    }
    fn select_tab(&mut self, workspace: &mut Workspace, app: &mut App, id: u64) {
        if self.busy(workspace) {
            workspace.message = Some("Wait for pending edits before changing tabs.".into());
            return;
        }
        self.save_current(workspace);
        if self
            .controller
            .as_mut()
            .is_some_and(|controller| controller.activate(id).is_ok())
        {
            self.install_views(workspace);
            if let Some(index) = self.tab_index(workspace, id) {
                app.active = index;
            }
            if !self.tab_hits.iter().any(|hit| hit.id == id) {
                if let Some(controller) = &self.controller {
                    let pane = controller.active_pane();
                    self.tab_offset[pane as usize] = controller
                        .pane_tabs(pane)
                        .position(|tab| tab.id == id)
                        .unwrap_or(0);
                }
            }
        }
    }
    pub(super) fn active_editor<'a>(
        &'a self,
        workspace: &'a Workspace,
        fallback: usize,
    ) -> Option<&'a SharedEditorView> {
        if self.pane() == 1 {
            self.secondary.as_ref().map(|editor| &**editor)
        } else {
            workspace.editors.get(fallback).map(|editor| &**editor)
        }
    }
    pub(super) fn active_workspace_editor<'a>(
        &'a self,
        workspace: &'a Workspace,
        fallback: usize,
    ) -> Option<&'a WorkspaceEditor> {
        if self.pane() == 1 {
            self.secondary.as_ref()
        } else {
            workspace.editors.get(fallback)
        }
    }
    pub(super) fn prepare_fold_target(&mut self, workspace: &mut Workspace) {
        self.sync_documents(workspace);
        self.install_views(workspace);
    }
    pub(super) fn record_fold_target(&mut self) {
        self.fold_target = self.controller.as_ref().and_then(|controller| {
            controller
                .active_tab(controller.active_pane())
                .map(|tab| (controller.active_pane(), tab))
        });
    }
    pub(super) fn cancel_fold_target(&mut self) {
        self.fold_target = None;
    }
    pub(super) fn apply_fold_result(
        &mut self,
        workspace: &mut Workspace,
        snapshot: &ViewSnapshot,
        folds: Vec<bareline_syntax::folding::Fold>,
        level: usize,
        partial: bool,
    ) {
        let Some((pane, tab)) = self.fold_target else {
            return;
        };
        if self.loaded_tabs[pane as usize] != Some(tab) {
            self.fold_target = None;
            return;
        }
        let index = self.tab_index(workspace, tab);
        let editor = if pane == 1 {
            self.secondary.as_mut()
        } else {
            index.and_then(|index| workspace.editors.get_mut(index))
        };
        let mut applied = false;
        if let Some(WorkspaceEditor::Resident(editor)) = editor {
            if editor.snapshot().same_document(snapshot)
                && editor.snapshot().revision == snapshot.revision
            {
                editor.set_known_folds(folds, level, partial);
                applied = true;
            }
        }
        if applied && pane == 1 {
            if let (Some(index), Some(WorkspaceEditor::Resident(peer))) = (index, &self.secondary) {
                workspace.editors[index].sync_fold_metadata_from(peer);
            }
        }
        if !partial {
            self.fold_target = None;
        }
    }
    pub(super) fn active_workspace_editor_mut<'a>(
        &'a mut self,
        workspace: &'a mut Workspace,
        fallback: usize,
    ) -> Option<&'a mut WorkspaceEditor> {
        if self.pane() == 1 {
            self.secondary.as_mut()
        } else {
            workspace.editors.get_mut(fallback)
        }
    }
    pub(super) fn active_editor_mut<'a>(
        &'a mut self,
        workspace: &'a mut Workspace,
        fallback: usize,
    ) -> Option<&'a mut SharedEditorView> {
        if self.pane() == 1 {
            self.secondary.as_mut().map(|editor| &mut **editor)
        } else {
            workspace
                .editors
                .get_mut(fallback)
                .map(|editor| &mut **editor)
        }
    }
    pub(super) fn history_available(
        &self,
        workspace: Option<&Workspace>,
        active: usize,
        undo: bool,
    ) -> bool {
        if self.open() && self.pane() == 1 {
            return self
                .secondary
                .as_ref()
                .is_some_and(|e| if undo { e.can_undo() } else { e.can_redo() });
        }
        workspace
            .and_then(|w| w.editors.get(active))
            .is_some_and(|e| if undo { e.can_undo() } else { e.can_redo() })
    }

    pub(super) fn annotate_context(&self, context: &mut bareline_commands::CommandContext) {
        if let Some(controller) = &self.controller {
            context
                .states
                .entry(CommandId("view.tabs.vertical"))
                .or_default()
                .checked = controller.vertical_tabs;
            context
                .states
                .entry(CommandId("view.sync_horizontal"))
                .or_default()
                .checked = controller.sync_horizontal;
            context
                .states
                .entry(CommandId("view.tabs.pin"))
                .or_default()
                .checked = controller
                .active_tab(controller.active_pane())
                .and_then(|id| controller.tab(id))
                .is_some_and(|tab| tab.pinned);
        }
        for id in [
            "view.close_split",
            "view.focus_other",
            "view.sync_vertical",
            "view.sync_horizontal",
        ] {
            let state = context.states.entry(CommandId(id)).or_default();
            state.enabled = self.open();
            if !self.open() {
                state.disabled_reason = Some("Open a split view first.".into());
            }
        }
        for (id, checked) in [
            (
                "view.split_vertical",
                self.open()
                    && self
                        .controller
                        .as_ref()
                        .is_some_and(|controller| controller.orientation == Orientation::Vertical),
            ),
            (
                "view.split_horizontal",
                self.open()
                    && self.controller.as_ref().is_some_and(|controller| {
                        controller.orientation == Orientation::Horizontal
                    }),
            ),
            (
                "view.sync_vertical",
                self.controller
                    .as_ref()
                    .is_some_and(|controller| controller.sync_vertical),
            ),
        ] {
            context.states.entry(CommandId(id)).or_default().checked = checked;
        }
    }
    pub(super) fn compare_pair(
        &mut self,
        workspace: &mut Workspace,
        left: usize,
        right: usize,
    ) -> bool {
        if self.busy(workspace)
            || [left, right]
                .iter()
                .any(|&index| workspace.editors.get(index).is_none())
        {
            return false;
        }
        self.split(workspace, left, Orientation::Vertical);
        let document = self
            .documents
            .iter()
            .find(|binding| binding.matches(&workspace.editors[right]))
            .map(DocumentBinding::id)
            .unwrap();
        if let Some(controller) = &mut self.controller {
            if let Some(id) = controller.active_tab(1) {
                let _ = controller.assign_document(id, document);
            }
        }
        self.loaded_tabs = [None, None];
        self.install_views(workspace);
        self.compare = true;
        true
    }
    pub(super) fn close_compare(&mut self, workspace: &mut Workspace) {
        self.compare = false;
        self.alignment = None;
        self.collapse(workspace, false);
    }
    pub(super) fn compare_geometry(&self) -> [Option<Rect>; 2] {
        self.bounds
    }
    pub(super) fn compare_snapshots(&self, workspace: &Workspace) -> Option<[ViewSnapshot; 2]> {
        Some([
            workspace
                .editors
                .get(self.primary_index(workspace)?)?
                .snapshot()
                .clone(),
            self.secondary.as_ref()?.snapshot().clone(),
        ])
    }
    pub(super) fn compare_selections(&self, workspace: &Workspace) -> Option<[bareline_editor_surface::Selection;2]> {
        let primary = workspace.editors.get(self.primary_index(workspace)?)?;
        let secondary = self.secondary.as_ref()?;
        let selection = |editor:&WorkspaceEditor| match editor {
            WorkspaceEditor::Paged(paged) => { let (anchor,caret)=paged.global_selection(); bareline_editor_surface::Selection {anchor:anchor.0,caret:caret.0} },
            editor => editor.selection,
        };
        Some([selection(primary),selection(secondary)])
    }
    pub(super) fn compare_layout_range(
        &self,
        workspace: &Workspace,
        side: usize,
        layout: bareline_renderer::LayoutId,
    ) -> Option<std::ops::Range<bareline_document::TextOffset>> {
        let editor = if side == 0 {
            &**workspace.editors.get(self.primary_index(workspace)?)?
        } else if side == 1 {
            &**self.secondary.as_ref()?
        } else {
            return None;
        };
        editor.layout_range(layout)
    }
    pub(super) fn compare_source_layout_range(&self, workspace: &Workspace, side: usize, layout: bareline_renderer::LayoutId) -> Option<std::ops::Range<bareline_document::TextOffset>> {
        let editor=if side==0 {workspace.editors.get(self.primary_index(workspace)?)?} else if side==1 {self.secondary.as_ref()?} else {return None;};
        let local=editor.layout_range(layout)?;
        match editor {
            WorkspaceEditor::Resident(_) => Some(local),
            WorkspaceEditor::Paged(paged) => {
                use bareline_editor_surface::paged_view::SourceAffinity;
                let start=paged.source_offset(local.start,SourceAffinity::After)?;
                let end=paged.source_offset(local.end,SourceAffinity::Before)?;
                (end.0.checked_sub(start.0)==Some(local.end.0-local.start.0)).then_some(start..end)
            }
        }
    }
    pub(super) fn compare_viewport_starts(&self, workspace: &Workspace) -> [usize; 2] {
        let start = |editor: &WorkspaceEditor| match editor {
            WorkspaceEditor::Paged(editor) => editor.viewport_start().0,
            _ => 0,
        };
        [
            self.primary_index(workspace)
                .map_or(0, |index| start(&workspace.editors[index])),
            self.secondary.as_ref().map_or(0, start),
        ]
    }
    pub(super) fn compare_scroll(&self, workspace: &Workspace) -> [f64; 2] {
        [
            self.primary_index(workspace)
                .map_or(0.0, |index| workspace.editors[index].scroll_y),
            self.secondary
                .as_ref()
                .map_or(0.0, |editor| editor.scroll_y),
        ]
    }
    pub(super) fn set_compare_alignment(
        &mut self,
        alignment: Option<bareline_app::views::AlignmentMap>,
    ) {
        self.alignment = alignment;
    }
    pub(super) fn compare_navigate(
        &mut self,
        workspace: &mut Workspace,
        left: bareline_document::TextOffset,
        right: bareline_document::TextOffset,
    ) {
        let first = self.primary_index(workspace);
        for (offset, editor) in [
            (
                left,
                first.and_then(|index| workspace.editors.get_mut(index)),
            ),
            (right, self.secondary.as_mut()),
        ] {
            if let Some(editor) = editor {
                if let WorkspaceEditor::Paged(editor) = editor {
                    if let Err(error) = editor.restore_selection(offset, offset) {
                        editor.error = Some(error);
                    }
                    continue;
                }
                let mut offset = offset.0.min(editor.snapshot().len());
                while !editor
                    .snapshot()
                    .is_boundary(bareline_document::TextOffset(offset))
                {
                    offset -= 1;
                }
                editor.selection.anchor = offset;
                editor.selection.caret = offset;
                editor.scroll_y = editor
                    .snapshot()
                    .line_at(bareline_document::TextOffset(offset))
                    .unwrap_or(0) as f64
                    * 19.2;
            }
        }
    }
    pub(super) fn restore_session(
        &mut self,
        workspace: &mut Workspace,
        app: &mut App,
        manifest: &bareline_file_io::session::SessionManifest,
        tabs: &[(u64, usize)],
    ) {
        let Ok(controller) = ViewController::from_session(manifest) else {
            return;
        };
        self.documents.clear();
        for (id, index) in tabs {
            if let (Some(tab), Some(editor)) = (
                manifest.tabs.iter().find(|tab| tab.id == *id),
                workspace.editors.get(*index),
            ) {
                if !self
                    .documents
                    .iter()
                    .any(|binding| binding.id() == tab.document_id)
                {
                    self.documents
                        .push(DocumentBinding::new(tab.document_id, editor));
                }
            }
        }
        self.next_document = manifest
            .documents
            .iter()
            .map(|document| document.id)
            .max()
            .unwrap_or(0);
        self.controller = Some(controller);
        self.loaded_tabs = [None, None];
        self.install_views(workspace);
        if let Some(id) = manifest
            .active_tab
            .and_then(|id| self.tab_index(workspace, id))
        {
            app.active = id;
        }
    }
    pub(super) fn capture_session(
        &self,
        workspace: &Workspace,
        manifest: &mut bareline_file_io::session::SessionManifest,
        tabs: &[(usize, u64)],
    ) {
        let Some(mut controller) = self.controller.clone() else {
            return;
        };
        self.save_view_states(workspace, &mut controller);
        let remap: Vec<_> = self
            .documents
            .iter()
            .filter_map(|binding| {
                let index = self.document_index(workspace, binding.id())?;
                let (_, tab_id) = tabs.iter().find(|(i, _)| *i == index)?;
                let document_id = manifest
                    .tabs
                    .iter()
                    .find(|tab| tab.id == *tab_id)?
                    .document_id;
                Some((binding.id(), document_id))
            })
            .collect();
        let retained: Vec<_> = manifest
            .tabs
            .iter()
            .filter(|tab| !remap.iter().any(|(_, id)| *id == tab.document_id))
            .cloned()
            .collect();
        controller.write_session(manifest);
        manifest.tabs.retain_mut(|tab| {
            if let Some((_, id)) = remap.iter().find(|(old, _)| *old == tab.document_id) {
                tab.document_id = *id;
                true
            } else {
                false
            }
        });
        manifest.mru = manifest
            .mru
            .iter()
            .filter_map(|old| remap.iter().find(|(id, _)| id == old).map(|(_, id)| *id))
            .collect();
        for mut tab in retained {
            if manifest.tabs.iter().any(|existing| existing.id == tab.id) {
                // Failed or deferred restores must retain a tab even if a new live view
                // reused the temporary capture ID. A bounded free ID avoids overflow.
                tab.id = (1..=10_001)
                    .find(|id| !manifest.tabs.iter().any(|existing| existing.id == *id))
                    .unwrap();
            }
            tab.view.split = 0;
            if !manifest.mru.contains(&tab.document_id) {
                manifest.mru.push(tab.document_id);
            }
            manifest.tabs.push(tab);
        }
        manifest.tabs.sort_by_key(|tab| !tab.pinned);
    }
    pub(super) fn set_secondary_focused(&mut self, focused: bool) {
        if let Some(editor) = &mut self.secondary {
            editor.set_focused(focused);
        }
    }
    pub(super) fn reset_secondary_caret_blink(&mut self) {
        if let Some(editor) = &mut self.secondary {
            editor.reset_caret_blink();
        }
    }
    pub(super) fn secondary_blink_deadline(&self) -> Option<Instant> {
        self.secondary
            .as_ref()
            .and_then(|editor| editor.blink_deadline())
    }
    pub(super) fn tick_secondary_caret_blink(&mut self, now: Instant) -> bool {
        self.secondary
            .as_mut()
            .is_some_and(|editor| editor.tick_caret_blink(now))
    }
    pub(super) fn pending_edits(&self) -> bool {
        !self.queued.is_empty() || self.secondary.as_ref().is_some_and(WorkspaceEditor::busy)
    }
    pub(super) fn take_acknowledged_inputs(&mut self) -> Vec<Input> {
        self.secondary
            .as_mut()
            .map(|editor| editor.take_acknowledged_inputs())
            .unwrap_or_default()
    }
    pub(super) fn take_ordered_receipts(
        &mut self,
    ) -> Vec<bareline_editor_surface::power::consumer::OrderedReceipt> {
        self.secondary
            .as_mut()
            .map(|editor| editor.take_ordered_receipts())
            .unwrap_or_default()
    }
    pub(super) fn take_acknowledged_commands(
        &mut self,
    ) -> Vec<(String, std::collections::BTreeMap<String, String>)> {
        self.secondary
            .as_mut()
            .map(|editor| editor.take_acknowledged_commands())
            .unwrap_or_default()
    }
    fn open(&self) -> bool {
        self.secondary.is_some() && self.controller.as_ref().is_some_and(|c| c.split)
    }
    pub(super) fn active_syntax_result<'a>(&'a self, workspace:&'a Workspace)->Option<&'a bareline_syntax::SyntaxResult> {
        if self.secondary.is_none() {return workspace.syntax_result();}
        let pane=self.pane() as usize;
        let editor=if pane==1 {self.secondary.as_ref()?} else {workspace.editors.get(self.primary_index(workspace)?)?};
        self.styling[pane].syntax_view(editor).result
    }
    pub(super) fn pane_token(&self, pane: usize) -> Option<u64> {
        self.loaded_tabs.get(pane).copied().flatten()
    }
    pub(super) fn pane(&self) -> u32 {
        self.controller
            .as_ref()
            .map_or(0, ViewController::active_pane)
    }
    fn index_of(workspace: &Workspace, document: &ViewSnapshot) -> Option<usize> {
        workspace
            .editors
            .iter()
            .position(|e| e.snapshot().same_document(document))
    }
    pub(super) fn primary_index(&self, workspace: &Workspace) -> Option<usize> {
        if let Some(index) = self.loaded_tabs[0].and_then(|id| self.tab_index(workspace, id)) {
            return Some(index);
        }
        self.primary
            .as_ref()
            .and_then(|s| Self::index_of(workspace, s))
    }
    fn secondary_index(&self, workspace: &Workspace) -> Option<usize> {
        self.loaded_tabs[1].and_then(|id| self.tab_index(workspace, id))
    }
    fn busy(&self, workspace: &Workspace) -> bool {
        !self.queued.is_empty()
            || self.secondary.as_ref().is_some_and(WorkspaceEditor::busy)
            || self
                .primary_index(workspace)
                .is_some_and(|i| workspace.editors[i].busy())
    }
    pub(super) fn pump(&mut self, workspace: &mut Workspace) -> bool {
        self.sync_documents(workspace);
        let mut changed = self
            .styling
            .iter_mut()
            .fold(false, |changed, styling| styling.pump() | changed);
        changed |= self.secondary.as_mut().is_some_and(WorkspaceEditor::pump);
        if let Some(index) = self.secondary_index(workspace)
            && let Some(peer) = &mut self.secondary
        {
            match (&mut workspace.editors[index], &mut *peer) {
                (WorkspaceEditor::Resident(primary), WorkspaceEditor::Resident(secondary)) => {
                    if secondary.snapshot().revision.0 > primary.snapshot().revision.0 {
                        changed |= primary.refresh_peer(secondary.snapshot());
                    } else if primary.snapshot().revision.0 > secondary.snapshot().revision.0 {
                        changed |= secondary.refresh_peer(primary.snapshot());
                    }
                    secondary.sync_saved_from(primary);
                    secondary.sync_fold_metadata_from(primary);
                }
                (WorkspaceEditor::Paged(primary), WorkspaceEditor::Paged(secondary)) => {
                    changed |= primary.refresh_peer();
                    changed |= secondary.refresh_peer();
                }
                _ => {}
            }
            peer.theme = workspace.editors[index].theme;
            let _ = peer.set_font_family(workspace.editors[index].font_family());
        }
        for pane in 0..2 {
            let index = self.primary_index(workspace);
            let editor = if pane == 1 {
                self.secondary.as_mut()
            } else {
                index.and_then(|index| workspace.editors.get_mut(index))
            };
            if let Some(editor) = editor {
                if !editor.busy() {
                    if let Some(mut pending) = self.pending_view_scroll[pane].take() {
                        if !pending.document.matches_state(editor) {
                            editor.error = Some(
                                "The document changed while its view was being restored.".into(),
                            );
                        } else if pending.state.scroll_byte.is_none()
                            && matches!(editor, WorkspaceEditor::Paged(paged) if paged.viewport_first_global_line().is_none())
                        {
                            if let WorkspaceEditor::Paged(paged) = &mut *editor {
                                let _ = paged.global_logical_scroll();
                            }
                            self.pending_view_scroll[pane] = Some(pending);
                        } else {
                            match finish_workspace_view_restore(editor, &pending.state, &mut pending.selection_token) {
                                Ok(false) => self.pending_view_scroll[pane] = Some(pending),
                                Ok(true) => changed = true,
                                Err(error) => { editor.error = Some(error); changed = true; }
                            }
                        }
                    }
                    if let Some(state) = self.pending_restore[pane].take() {
                        let document = DocumentBinding::new(0, editor);
                        match restore_workspace_view(editor, &state) {
                            Ok(()) if editor.paged() => {
                                self.pending_view_scroll[pane] =
                                    Some(PendingViewScroll { state, document, selection_token: None })
                            }
                            Ok(()) => {}
                            Err(error) => editor.error = Some(error),
                        }
                        changed = true;
                    }
                }
            } else {
                self.pending_restore[pane] = None;
            }
        }
        if self.open() && self.secondary_index(workspace).is_none() {
            self.collapse(workspace, false);
            changed = true;
        }
        let primary_busy = self
            .primary_index(workspace)
            .is_some_and(|i| workspace.editors[i].busy());
        if !primary_busy
            && !self.secondary.as_ref().is_some_and(WorkspaceEditor::busy)
            && let Some(queued) = self.queued.pop_front()
        {
            if queued.pane == 0 {
                if let Some(index) = self.primary_index(workspace) {
                    if queued.document.matches(&workspace.editors[index]) {
                        workspace.editors[index].enqueue(queued.input);
                    } else {
                        workspace.message =
                            Some("The view changed before queued input could be applied.".into());
                    }
                }
            } else if let Some(editor) = &mut self.secondary {
                if queued.document.matches(editor) {
                    editor.enqueue(queued.input);
                } else {
                    workspace.message =
                        Some("The view changed before queued input could be applied.".into());
                }
            }
            changed = true;
        }
        changed |= self.flush_sync_scroll(workspace);
        changed
    }
    fn input(&mut self, workspace: &mut Workspace, pane: u32, input: Input) {
        self.pump(workspace);
        let document = if pane == 1 {
            self.secondary
                .as_ref()
                .map(|editor| DocumentBinding::new(0, editor))
        } else {
            self.primary_index(workspace)
                .map(|i| DocumentBinding::new(0, &workspace.editors[i]))
        };
        let Some(document) = document else {
            return;
        };
        if self.queued.len() >= 256 {
            workspace.message =
                Some("Split-view input queue is full; wait for the pending edit.".into());
            return;
        }
        self.queued.push_back(QueuedInput {
            pane,
            document,
            input,
        });
        self.pump(workspace);
    }
    fn split(&mut self, workspace: &mut Workspace, index: usize, orientation: Orientation) {
        self.sync_documents(workspace);
        if self.busy(workspace) {
            workspace.message = Some("Wait for pending edits before changing views.".into());
            return;
        }
        if workspace.editors.get(index).is_none() {
            workspace.message = Some("The document is no longer available.".into());
            return;
        }
        self.save_current(workspace);
        let document = self
            .documents
            .iter()
            .find(|binding| binding.matches(&workspace.editors[index]))
            .map(DocumentBinding::id)
            .unwrap();
        let controller = self.controller.as_mut().unwrap();
        let id = controller
            .tabs()
            .iter()
            .find(|tab| tab.document_id == document && tab.view.split == controller.active_pane())
            .or_else(|| {
                controller
                    .tabs()
                    .iter()
                    .find(|tab| tab.document_id == document)
            })
            .map(|tab| tab.id)
            .unwrap();
        let _ = controller.activate(id);
        if !controller.split {
            let _ = controller.clone_to_other(id);
        }
        controller.orientation = orientation;
        controller.split = true;
        self.install_views(workspace);
    }
    fn collapse(&mut self, workspace: &mut Workspace, keep_secondary: bool) {
        for editor in &mut workspace.editors {
            match editor {
                WorkspaceEditor::Paged(editor) => {
                    let _ = editor.set_global_spacers(&[]);
                }
                editor => {
                    let _ = editor.set_view_spacers(&[]);
                }
            }
        }
        self.pending_sync = None;
        self.applied_spacers = [None, None];
        self.save_current(workspace);
        if let Some(controller) = &mut self.controller {
            if keep_secondary {
                if let Some(id) = controller.active_tab(1) {
                    let _ = controller.activate(id);
                }
            }
            controller.collapse();
        }
        self.loaded_tabs = [None, None];
        self.install_views(workspace);
        self.bounds = [None, None];
        self.splitter = None;
        self.dragging = false;
    }
    fn clone_active(&mut self, workspace: &mut Workspace, app: &mut App) {
        self.sync_documents(workspace);
        if self.busy(workspace) {
            workspace.message = Some("Wait for pending edits before cloning a view.".into());
            return;
        }
        if workspace.editors.get(app.active).is_none() {
            workspace.message = Some("The document is no longer available.".into());
            return;
        }
        self.save_current(workspace);
        if let Some(controller) = &mut self.controller {
            if let Some(id) = controller.active_tab(controller.active_pane()) {
                let _ = controller.clone_to_other(id);
            }
        }
        self.install_views(workspace);
        if let Some(id) = self
            .controller
            .as_ref()
            .and_then(|controller| controller.active_tab(controller.active_pane()))
            .and_then(|id| self.tab_index(workspace, id))
        {
            app.active = id;
        }
    }
    fn move_active(&mut self, workspace: &mut Workspace, app: &mut App) {
        self.sync_documents(workspace);
        if self.busy(workspace) {
            workspace.message = Some("Wait for pending edits before moving a view.".into());
            return;
        }
        self.save_current(workspace);
        if let Some(controller) = &mut self.controller {
            if let Some(id) = controller.active_tab(controller.active_pane()) {
                let _ = controller.move_to_other(id);
            }
            if controller.active_tab(0).is_none() || controller.active_tab(1).is_none() {
                controller.collapse();
            }
        }
        self.loaded_tabs = [None, None];
        self.install_views(workspace);
        if let Some(index) = self
            .controller
            .as_ref()
            .and_then(|controller| controller.active_tab(controller.active_pane()))
            .and_then(|id| self.tab_index(workspace, id))
        {
            app.active = index;
        }
    }
    pub(super) fn activate_watch_pane(&mut self, workspace: &Workspace, app: &mut App, pane: u32) -> bool {
        if pane > 1 || (pane == 1 && self.secondary.is_none()) { return false; }
        self.activate(workspace, app, pane);
        true
    }
    fn activate(&mut self, workspace: &Workspace, app: &mut App, pane: u32) {
        if let Some(controller) = &mut self.controller {
            if let Some(id) = controller.active_tab(pane) {
                let _ = controller.activate(id);
            }
        }
        if let Some(index) = if pane == 0 {
            self.primary_index(workspace)
        } else {
            self.secondary_index(workspace)
        } {
            app.active = index;
        }
    }
    fn close_tab(&mut self, workspace: &mut Workspace, app: &mut App, id: u64) {
        if self.busy(workspace) {
            workspace.message = Some("Wait for pending edits before closing a view.".into());
            return;
        }
        let Some(index) = self.tab_index(workspace, id) else {
            return;
        };
        self.save_current(workspace);
        let controller = self.controller.as_ref().unwrap();
        let document = controller.tab(id).unwrap().document_id;
        if controller
            .tabs()
            .iter()
            .filter(|tab| tab.document_id == document)
            .count()
            == 1
        {
            app.active = index;
            self.pending_close = Some(index);
            return;
        }
        let _ =
            self.controller
                .as_mut()
                .unwrap()
                .close(id, workspace.editors[index].dirty(), false);
        self.loaded_tabs = [None, None];
        self.install_views(workspace);
        if let Some(index) = self
            .controller
            .as_ref()
            .and_then(|controller| controller.active_tab(controller.active_pane()))
            .and_then(|id| self.tab_index(workspace, id))
        {
            app.active = index;
        }
    }
    fn sync_scroll(&mut self, workspace: &mut Workspace, pane: u32) {
        self.pending_sync = self.loaded_tabs[pane as usize].map(|tab| (pane, tab));
        self.flush_sync_scroll(workspace);
    }
    fn flush_sync_scroll(&mut self, workspace: &mut Workspace) -> bool {
        let Some((pane, tab)) = self.pending_sync.take() else {
            return false;
        };
        if self.loaded_tabs[pane as usize] != Some(tab) {
            return false;
        }
        let Some(controller) = &self.controller else {
            return false;
        };
        let vertical = controller.sync_vertical;
        let horizontal = controller.sync_horizontal;
        if !self.open() || (!vertical && !horizontal) {
            return false;
        }
        let primary = self.primary_index(workspace);
        let source = if pane == 1 {
            self.secondary.as_mut()
        } else {
            primary.and_then(|index| workspace.editors.get_mut(index))
        };
        let Some(source) = source else {
            return false;
        };
        let position = match source {
            WorkspaceEditor::Paged(editor) if vertical => match editor.global_logical_scroll() {
                bareline_editor_surface::paged_view::GlobalScrollPosition::Ready(
                    line,
                    fraction,
                    x,
                ) => (line, fraction, x),
                bareline_editor_surface::paged_view::GlobalScrollPosition::Pending => {
                    self.pending_sync = Some((pane, tab));
                    return false;
                }
            },
            editor => editor.logical_scroll(),
        };
        let update = self.controller.as_mut().unwrap().begin_scroll(
            pane,
            ScrollPosition {
                line: position.0,
                fraction: position.1,
                x: position.2,
            },
            self.alignment.as_ref(),
        );
        let Ok(Some(update)) = update else {
            return false;
        };
        let target = if update.pane == 1 {
            self.secondary.as_mut()
        } else {
            primary.and_then(|index| workspace.editors.get_mut(index))
        };
        if let Some(target) = target {
            let old = target.logical_scroll();
            let x = if horizontal { update.position.x } else { old.2 };
            if vertical {
                match target {
                    WorkspaceEditor::Paged(editor) => {
                        if let Err(error) = editor.request_global_scroll(
                            update.position.line,
                            update.position.fraction,
                            x,
                        ) {
                            editor.error = Some(error);
                        }
                    }
                    editor => {
                        editor.set_logical_scroll(update.position.line, update.position.fraction, x)
                    }
                }
            } else {
                target.scroll_horizontal(x - old.2);
            }
        }
        true
    }
    pub(super) fn draw(
        &mut self,
        workspace: &mut Workspace,
        app: &mut App,
        renderer: &mut impl TextBackend,
        width: f32,
        height: f32,
        ops: &mut Vec<DrawOp>,
        notify: std::sync::Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Option<Rect>, LayoutError> {
        for mut peer in self.retired.drain(..) {
            peer.release_layouts(renderer);
        }
        self.pump(workspace);
        let desired = self.controller.as_ref().and_then(|controller| {
            controller
                .pane_tabs(controller.active_pane())
                .find(|tab| self.document_index(workspace, tab.document_id) == Some(app.active))
                .or_else(|| {
                    controller.tabs().iter().find(|tab| {
                        self.document_index(workspace, tab.document_id) == Some(app.active)
                    })
                })
                .map(|tab| tab.id)
        });
        if let Some(id) = desired {
            if self.loaded_tabs[self.pane() as usize] != Some(id) {
                self.select_tab(workspace, app, id);
            }
        }
        self.install_views(workspace);
        self.tab_hits.clear();
        self.tab_nav.clear();
        self.tab_strips = [None, None];
        let vertical = self
            .controller
            .as_ref()
            .is_some_and(|controller| controller.vertical_tabs);
        if !self.open() {
            let inset = if vertical {
                176.0f32.min(width * 0.4)
            } else {
                0.0
            };
            let mut local = Vec::new();
            let caret = workspace.draw(
                app.active,
                renderer,
                (width - inset).max(0.0),
                height,
                &mut local,
            )?;
            ops.extend(local.into_iter().map(|op| translate(op, inset, 0.0)));
            self.bounds = [
                Some(rect(inset, 0.0, (width - inset).max(0.0), height - 24.0)),
                None,
            ];
            self.draw_tab_strip(
                workspace,
                0,
                if vertical {
                    rect(0.0, 0.0, inset, height - 24.0)
                } else {
                    rect(0.0, 0.0, width, TAB_HEIGHT)
                },
                vertical,
                ops,
            );
            self.draw_mru(workspace, width, height, ops);
            return Ok(caret.map(|caret| rect(caret.x + inset, caret.y, caret.width, caret.height)));
        }
        let pane = self.pane();
        let Some(first) = self.primary_index(workspace) else {
            self.collapse(workspace, true);
            return workspace.draw(app.active, renderer, width, height, ops);
        };
        let find_height = if workspace.find.open {
            workspace.find.height()
        } else {
            0.0
        };
        let panel_height = workspace.search_panel.height();
        let compare_height = if self.compare { 44.0 } else { 0.0 };
        let geometry = self.controller.as_ref().unwrap().geometry(rect(
            0.0,
            TAB_HEIGHT + find_height + compare_height,
            width,
            (height - TAB_HEIGHT - find_height - compare_height - 24.0 - panel_height).max(0.0),
        ));
        self.bounds = geometry.panes;
        self.splitter = geometry.splitter;
        let strips = self.bounds.map(|bounds| {
            bounds.map(|bounds| {
                if vertical {
                    rect(
                        bounds.x,
                        bounds.y,
                        176.0f32.min(bounds.width * 0.4),
                        bounds.height,
                    )
                } else {
                    rect(bounds.x, bounds.y, bounds.width, TAB_HEIGHT)
                }
            })
        });
        if vertical {
            for bounds in self.bounds.iter_mut().flatten() {
                let inset = 176.0f32.min(bounds.width * 0.4);
                bounds.x += inset;
                bounds.width -= inset;
            }
        }
        let titles = workspace.titles();
        let second = self.secondary_index(workspace).unwrap_or(first);
        let mut active_caret = None;
        let mut status = Vec::new();
        self.draw_tab_strip(
            workspace,
            pane,
            rect(0.0, 0.0, width, TAB_HEIGHT),
            false,
            ops,
        );
        let paths = [
            workspace.path(first).map(std::path::Path::to_path_buf),
            workspace.path(second).map(std::path::Path::to_path_buf),
        ];
        for side in 0..2 {
            let Some(bounds) = self.bounds[side] else {
                continue;
            };
            let editor = if side == 0 {
                &mut workspace.editors[first]
            } else {
                self.secondary.as_mut().unwrap()
            };
            let spacers = self
                .alignment
                .as_ref()
                .map(|alignment| alignment.spacers(side))
                .unwrap_or_default();
            if self.applied_spacers[side].as_ref() != Some(&spacers) {
                let result = match &mut *editor {
                    WorkspaceEditor::Paged(editor) => editor.set_global_spacers(&spacers),
                    editor => editor.set_view_spacers(&spacers),
                };
                if let Err(error) = result {
                    editor.error = Some(error);
                }
                self.applied_spacers[side] = Some(spacers);
            }
            // EditorSurface already reserves TAB_HEIGHT for this pane's header.
            editor.top_inset = 0.0;
            editor.bottom_inset = 0.0;
            let paged = editor.paged();
            editor.set_external_scrollbar(paged);
            let mut local = Vec::new();
            let local_height = bounds.height + 24.0;
            self.styling[side].prepare_view(editor, paths[side].as_deref(), notify.clone());
            let syntax = self.styling[side].syntax_view(editor);
            let mut caret = if bareline_app::workspace::paint_paged_pending(editor, bounds.width, local_height, workspace.theme, &mut local) {
                None
            } else { editor.draw_styled(renderer, bounds.width, local_height, &mut local, syntax)? };
            if let WorkspaceEditor::Paged(paged) = &mut *editor {
                if let Err(error)=paged.refine_horizontal_viewport(renderer,bounds.width) {paged.error=Some(error);}
            }
            if matches!(&*editor, WorkspaceEditor::Paged(paged) if !paged.paged_frame_state().ready) {
                local.clear();
                bareline_app::workspace::paint_paged_pending(editor,bounds.width,local_height,workspace.theme,&mut local);
                caret=None;
            }
            if matches!(&*editor, WorkspaceEditor::Paged(paged) if !paged.caret_in_viewport()) {
                if let Some(rect) = caret.take() { local.retain(|op| !matches!(op, DrawOp::Fill(bounds, _) if *bounds == rect)); }
            }
            self.styling[side].prepare_view(editor, paths[side].as_deref(), notify.clone());
            if side as u32 == pane {
                status = local
                    .iter()
                    .filter_map(|op| {
                        if let DrawOp::Text { origin, text, .. } = op {
                            (origin.y == local_height - 20.0).then(|| text.clone())
                        } else {
                            None
                        }
                    })
                    .collect();
                active_caret =
                    caret.map(|r| rect(r.x + bounds.x, r.y + bounds.y, r.width, r.height));
            } else if let Some(caret) = caret {
                local.retain(|op| !matches!(op, DrawOp::Fill(r, _) if *r == caret));
            }
            ops.push(DrawOp::PushClip(bounds));
            ops.extend(
                local
                    .into_iter()
                    .map(|op| translate(op, bounds.x, bounds.y)),
            );
            ops.push(DrawOp::Fill(
                rect(bounds.x, bounds.y, bounds.width, TAB_HEIGHT),
                CHROME,
            ));
            text(
                ops,
                bounds.x + 12.0,
                bounds.y + 8.0,
                titles
                    .get(if side == 0 { first } else { second })
                    .map_or("Document", String::as_str),
                13.0,
                if side as u32 == pane { TEXT } else { MUTED },
            );
            if side as u32 == pane {
                ops.push(DrawOp::Fill(
                    rect(bounds.x, bounds.y + TAB_HEIGHT - 2.0, bounds.width, 2.0),
                    ACCENT,
                ));
            }
            ops.push(DrawOp::PopClip);
        }
        if let Some(splitter) = self.splitter {
            ops.push(DrawOp::Fill(splitter, BORDER));
        }
        for (pane, bounds) in strips.into_iter().enumerate() {
            if let Some(bounds) = bounds {
                self.draw_tab_strip(workspace, pane as u32, bounds, vertical, ops);
            }
        }
        ops.push(DrawOp::Fill(rect(0.0, height - 24.0, width, 24.0), CHROME));
        for (index, label) in status.into_iter().take(6).enumerate() {
            text(
                ops,
                12.0 + width * index as f32 / 6.0,
                height - 20.0,
                label,
                13.0,
                MUTED,
            );
        }
        if let Some(caret) =
            workspace
                .find
                .draw_with_theme(renderer, width, workspace.theme, ops)?
        {
            active_caret = Some(caret);
        }
        let labels: Vec<_> = workspace
            .editors
            .iter()
            .zip(titles)
            .map(|(e, title)| (e.snapshot().clone(), title))
            .collect();
        if let Some(caret) = workspace
            .search_panel
            .draw(renderer, width, height, &labels, ops)?
        {
            active_caret = Some(caret);
        }
        self.draw_mru(workspace, width, height, ops);
        Ok(active_caret)
    }
}

fn translate(op: DrawOp, x: f32, y: f32) -> DrawOp {
    let r = |r: Rect| rect(r.x + x, r.y + y, r.width, r.height);
    let p = |p: Point| Point {
        x: p.x + x,
        y: p.y + y,
    };
    match op {
        DrawOp::Fill(a, c) => DrawOp::Fill(r(a), c),
        DrawOp::Stroke(a, c, w) => DrawOp::Stroke(r(a), c, w),
        DrawOp::FillRounded(a, c, v) => DrawOp::FillRounded(r(a), c, v),
        DrawOp::StrokeRounded(a, c, v, w) => DrawOp::StrokeRounded(r(a), c, v, w),
        DrawOp::Text {
            origin,
            text,
            size,
            color,
        } => DrawOp::Text {
            origin: p(origin),
            text,
            size,
            color,
        },
        DrawOp::Layout {
            origin,
            layout,
            color,
        } => DrawOp::Layout {
            origin: p(origin),
            layout,
            color,
        },
        DrawOp::PushClip(a) => DrawOp::PushClip(r(a)),
        DrawOp::PopClip => DrawOp::PopClip,
        DrawOp::Image {
            image,
            destination,
            opacity,
        } => DrawOp::Image {
            image,
            destination: r(destination),
            opacity,
        },
        DrawOp::PushLayer { bounds, opacity } => DrawOp::PushLayer {
            bounds: r(bounds),
            opacity,
        },
        DrawOp::PopLayer => DrawOp::PopLayer,
        DrawOp::Line {
            from,
            to,
            color,
            width,
        } => DrawOp::Line {
            from: p(from),
            to: p(to),
            color,
            width,
        },
    }
}

fn scroll_workspace_view(editor: &mut WorkspaceEditor, delta: f64, height: f32) {
    match editor {
        WorkspaceEditor::Paged(editor) => {
            if let Err(error) = editor.scroll_viewport(delta, height) {
                editor.error = Some(error);
            }
        }
        editor => editor.scroll(delta, height),
    }
}
fn workspace_view_state(editor: &WorkspaceEditor) -> ViewState {
    let mut state = view_state(editor);
    if let WorkspaceEditor::Paged(paged) = editor {
        state.scroll_byte = Some(paged.viewport_start().0 as u64);
        state.folds = paged.persisted_global_folds();
        state.scroll_line = paged
            .viewport_first_global_line()
            .map_or(0, |first| first.saturating_add(state.scroll_line));
        let (anchor, caret) = paged.global_selection();
        state.anchor = anchor.0 as u64;
        state.caret = caret.0 as u64;
    }
    state
}
fn restore_workspace_view(editor: &mut WorkspaceEditor, state: &ViewState) -> Result<(), String> {
    editor.restore_session_language(state.language.as_ref());
    match editor {
        WorkspaceEditor::Resident(editor) => {
            restore_view(editor, state);
            Ok(())
        }
        WorkspaceEditor::Paged(editor) => {
            let anchor = usize::try_from(state.anchor)
                .map_err(|_| "Saved anchor exceeds this platform's range")?;
            let caret = usize::try_from(state.caret)
                .map_err(|_| "Saved caret exceeds this platform's range")?;
            if anchor > editor.snapshot().len() || caret > editor.snapshot().len() {
                return Err("Saved selection is outside the restored document.".into());
            }
            if let Some(byte) = state.scroll_byte {
                let byte = usize::try_from(byte)
                    .map_err(|_| "Saved viewport exceeds this platform's range")?;
                if byte > editor.snapshot().len() {
                    return Err("Saved viewport is outside the restored document.".into());
                }
                editor.request_viewport(bareline_document::TextOffset(byte))?;
            } else {
                editor.restore_selection(
                    bareline_document::TextOffset(anchor),
                    bareline_document::TextOffset(caret),
                )?;
            }
            editor.restore_global_folds(&state.folds);
            Ok(())
        }
    }
}
fn finish_workspace_view_restore(
    editor: &mut WorkspaceEditor,
    state: &ViewState,
    selection_token: &mut Option<u64>,
) -> Result<bool, String> {
    let WorkspaceEditor::Paged(editor) = editor else {
        restore_view(editor, state);
        return Ok(true);
    };
    if !editor.viewport_ready() { return Err("The saved viewport could not be loaded.".into()); }
    if state.scroll_byte.is_some() {
        let token = if let Some(token) = *selection_token { token } else {
            let anchor = usize::try_from(state.anchor).map_err(|_| "Saved anchor exceeds this platform's range")?;
            let caret = usize::try_from(state.caret).map_err(|_| "Saved caret exceeds this platform's range")?;
            let token = editor.restore_global_selection(bareline_document::TextOffset(anchor), bareline_document::TextOffset(caret), true)?;
            *selection_token = Some(token);
            token
        };
        use bareline_editor_surface::paged_view::SelectionRestoreStatus;
        match editor.selection_restore_status(token) {
            SelectionRestoreStatus::Pending => return Ok(false),
            SelectionRestoreStatus::Failed(error) => return Err(error),
            SelectionRestoreStatus::Superseded => return Err("Saved selection restoration was superseded.".into()),
            SelectionRestoreStatus::Applied => {}
        }
        editor.surface.set_logical_scroll(0, 0.0, state.scroll_x as f64);
        editor.surface.scroll_y = f64::from_bits(state.scroll_y_bits);
    } else {
        editor.request_global_scroll(state.scroll_line, 0.0, state.scroll_x as f64)?;
    }
    Ok(true)
}

fn view_state(editor: &SharedEditorView) -> ViewState {
    let (line, _, x) = editor.logical_scroll();
    ViewState {
        language: editor.session_language_selection(),
        anchor: editor.selection.anchor as u64,
        caret: editor.selection.caret as u64,
        scroll_line: line,
        scroll_x: x.clamp(0.0, u32::MAX as f64) as u32,
        scroll_y_bits: editor.scroll_y.max(0.0).to_bits(),
        folds: editor.persisted_folds(),
        ..Default::default()
    }
}
fn restore_view(editor: &mut SharedEditorView, state: &ViewState) {
    editor.restore_session_language(state.language.as_ref());
    let bound = |offset: u64| {
        let mut offset = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(editor.snapshot().len());
        while !editor
            .snapshot()
            .is_boundary(bareline_document::TextOffset(offset))
        {
            offset -= 1;
        }
        offset
    };
    let anchor = bound(state.anchor);
    let caret = bound(state.caret);
    editor.selection.anchor = anchor;
    editor.selection.caret = caret;
    if let Err(error) = editor.set_selections(editor.selection.into()) {
        editor.error = Some(error);
        return;
    }
    editor.set_logical_scroll(state.scroll_line, 0.0, state.scroll_x as f64);
    editor.scroll_y = f64::from_bits(state.scroll_y_bits);
    editor.restore_folds(&state.folds);
}

fn mru_visible_rows(bounds: Rect) -> usize {
    ((bounds.height / TAB_HEIGHT).floor() as usize).clamp(1, 12)
}
const ACCESS_TAB_BASE: u64 = 0x1000_0000_0000_0000;
const ACCESS_NAV_BASE: u64 = 0x2000_0000_0000_0000;
const ACCESS_MRU_BASE: u64 = 0x3000_0000_0000_0000;
fn access_tab_id(tab: u64) -> Option<u64> {
    tab.checked_mul(2)
        .and_then(|id| id.checked_add(ACCESS_TAB_BASE))
        .filter(|id| *id < ACCESS_NAV_BASE)
}
impl Shell {
    pub(super) fn views_accessibility_nodes(
        &self,
    ) -> Vec<bareline_platform::accessibility::AccessibilityNode> {
        use bareline_platform::accessibility::{AccessibilityNode, AccessibilityRole};
        let Some(workspace) = &self.workspace else {
            return Vec::new();
        };
        let Some(controller) = &self.views.controller else {
            return Vec::new();
        };
        let offset = self.editor_bounds();
        let bounds = |rect: Rect| {
            [
                (rect.x + offset.x) as f64,
                (rect.y + offset.y) as f64,
                rect.width as f64,
                rect.height as f64,
            ]
        };
        let titles = workspace.titles();
        let mut nodes = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for hit in &self.views.tab_hits {
            if !seen.insert(hit.id) {
                continue;
            }
            let Some(id) = access_tab_id(hit.id) else {
                continue;
            };
            let Some(tab) = controller.tab(hit.id) else {
                continue;
            };
            let Some(index) = self.views.tab_index(workspace, hit.id) else {
                continue;
            };
            let title = titles.get(index).cloned().unwrap_or_default();
            let name = format!(
                "{}{}, pane {}{}",
                title,
                if tab.pinned { ", pinned" } else { "" },
                hit.pane + 1,
                if workspace.editors[index].dirty() {
                    ", modified"
                } else {
                    ""
                }
            );
            nodes.push(AccessibilityNode {
                id,
                parent: 1,
                role: AccessibilityRole::Tab,
                name,
                value: controller
                    .tab_colors
                    .get(&hit.id)
                    .map(|color| format!("#{color:06x}")),
                bounds: bounds(hit.bounds),
                disabled: self.views.busy(workspace),
                selected: controller.active_tab(hit.pane) == Some(hit.id),
                expanded: None,
                focusable: true,
                invokable: true,
            });
            nodes.push(AccessibilityNode {
                id: id + 1,
                parent: id,
                role: AccessibilityRole::Button,
                name: format!("Close {title}"),
                value: None,
                bounds: bounds(hit.close),
                disabled: self.views.busy(workspace),
                selected: false,
                expanded: None,
                focusable: true,
                invokable: true,
            });
        }
        for (pane, forward, rect) in &self.views.tab_nav {
            let id = ACCESS_NAV_BASE + *pane as u64 * 2 + u64::from(*forward);
            if nodes.iter().any(|node| node.id == id) {
                continue;
            }
            nodes.push(AccessibilityNode {
                id,
                parent: 1,
                role: AccessibilityRole::Button,
                name: format!(
                    "{} tabs in pane {}",
                    if *forward { "Next" } else { "Previous" },
                    pane + 1
                ),
                value: None,
                bounds: bounds(*rect),
                disabled: false,
                selected: false,
                expanded: None,
                focusable: true,
                invokable: true,
            });
        }
        if let Some(popup) = &self.views.mru_popup {
            nodes.push(AccessibilityNode {
                id: ACCESS_MRU_BASE,
                parent: 1,
                role: AccessibilityRole::List,
                name: "Recent documents".into(),
                value: None,
                bounds: bounds(popup.bounds),
                disabled: false,
                selected: false,
                expanded: Some(true),
                focusable: false,
                invokable: false,
            });
            let start = popup
                .selected
                .saturating_sub(mru_visible_rows(popup.bounds).saturating_sub(1));
            for (row, tab) in popup
                .ids
                .iter()
                .skip(start)
                .take(mru_visible_rows(popup.bounds))
                .enumerate()
            {
                let Some(id) = access_tab_id(*tab).and_then(|id| id.checked_add(ACCESS_MRU_BASE))
                else {
                    continue;
                };
                let Some(index) = self.views.tab_index(workspace, *tab) else {
                    continue;
                };
                let row_bounds = rect(
                    popup.bounds.x,
                    popup.bounds.y + row as f32 * TAB_HEIGHT,
                    popup.bounds.width,
                    TAB_HEIGHT.min((popup.bounds.height - row as f32 * TAB_HEIGHT).max(0.0)),
                );
                if row_bounds.height == 0.0 {
                    continue;
                }
                nodes.push(AccessibilityNode {
                    id,
                    parent: ACCESS_MRU_BASE,
                    role: AccessibilityRole::ListItem,
                    name: titles.get(index).cloned().unwrap_or_default(),
                    value: None,
                    bounds: bounds(row_bounds),
                    disabled: self.views.busy(workspace),
                    selected: start + row == popup.selected,
                    expanded: None,
                    focusable: true,
                    invokable: true,
                });
            }
        }
        nodes
    }
    pub(super) fn views_accessibility_focus(&self) -> Option<u64> {
        if let Some(popup) = &self.views.mru_popup {
            return access_tab_id(*popup.ids.get(popup.selected)?)
                .and_then(|id| id.checked_add(ACCESS_MRU_BASE));
        }
        self.views.accessibility_focus.filter(|id| {
            self.views_accessibility_nodes()
                .iter()
                .any(|node| node.id == *id)
        })
    }
    pub(super) fn views_accessibility(
        &mut self,
        el: &winit::event_loop::ActiveEventLoop,
        action: &bareline_platform::accessibility::AccessibilityAction,
    ) -> bool {
        use bareline_platform::accessibility::AccessibilityAction;
        let (id, invoke) = match action {
            AccessibilityAction::Focus(id) => (*id, false),
            AccessibilityAction::Invoke(id) => (*id, true),
            _ => return false,
        };
        if let Some(popup) = &mut self.views.mru_popup {
            if let Some(position) = popup.ids.iter().position(|tab| {
                access_tab_id(*tab).and_then(|id| id.checked_add(ACCESS_MRU_BASE)) == Some(id)
            }) {
                popup.selected = position;
                if invoke {
                    let tab = popup.ids[position];
                    self.views.mru_popup = None;
                    if let Some(workspace) = &mut self.workspace {
                        self.views.select_tab(workspace, &mut self.app, tab);
                    }
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                return true;
            }
        }
        if let Some((pane, forward, _)) = self
            .views
            .tab_nav
            .iter()
            .find(|(pane, forward, _)| {
                ACCESS_NAV_BASE + *pane as u64 * 2 + u64::from(*forward) == id
            })
            .copied()
        {
            self.views.accessibility_focus = if invoke { None } else { Some(id) };
            if invoke {
                let offset = &mut self.views.tab_offset[pane as usize];
                *offset = if forward {
                    offset.saturating_add(1)
                } else {
                    offset.saturating_sub(1)
                };
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            return true;
        }
        let Some(hit) = self
            .views
            .tab_hits
            .iter()
            .find(|hit| access_tab_id(hit.id).is_some_and(|base| id == base || id == base + 1))
            .copied()
        else {
            return false;
        };
        let Some(workspace) = &mut self.workspace else {
            return false;
        };
        if self.views.busy(workspace) {
            return true;
        }
        self.views.accessibility_focus = if invoke { None } else { Some(id) };
        if invoke && access_tab_id(hit.id).is_some_and(|base| id == base + 1) {
            self.views.close_tab(workspace, &mut self.app, hit.id);
        } else {
            self.views.select_tab(workspace, &mut self.app, hit.id);
        }
        if self.views.pending_close.take().is_some() {
            self.dispatch(el, Action::Close);
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
}

impl Shell {
    fn tabs_dispatch(&mut self, id: &str) -> bool {
        if !id.starts_with("view.tabs.") {
            return false;
        }
        let Some(workspace) = &mut self.workspace else {
            return true;
        };
        self.views.sync_documents(workspace);
        self.views.save_current(workspace);
        let Some(controller) = &mut self.views.controller else {
            return true;
        };
        let active = controller.active_tab(controller.active_pane());
        match id {
            "view.tabs.vertical" => controller.vertical_tabs = !controller.vertical_tabs,
            "view.tabs.pin" => {
                if let Some(id) = active {
                    let pinned = controller.tab(id).unwrap().pinned;
                    let _ = controller.pin(id, !pinned);
                }
            }
            "view.tabs.color" => {
                if let Some(id) = active {
                    let colors = [0x36c9c6, 0xd19a66, 0xc678dd, 0x61afef];
                    let current = controller.tab_colors.get(&id).copied();
                    let next = current
                        .and_then(|color| colors.iter().position(|value| *value == color))
                        .map_or(Some(colors[0]), |index| colors.get(index + 1).copied());
                    let _ = controller.color(id, next);
                }
            }
            "view.tabs.sort_name" | "view.tabs.sort_descending" | "view.tabs.sort_path" => {
                let titles = workspace.titles();
                let labels: Vec<_> = self
                    .views
                    .documents
                    .iter()
                    .filter_map(|binding| {
                        workspace
                            .editors
                            .iter()
                            .position(|editor| binding.matches(editor))
                            .map(|index| {
                                (
                                    binding.id(),
                                    if id == "view.tabs.sort_path" {
                                        workspace
                                            .path(index)
                                            .map(|path| path.to_string_lossy().into_owned())
                                            .unwrap_or_else(|| titles[index].clone())
                                    } else {
                                        titles[index].clone()
                                    },
                                )
                            })
                    })
                    .collect();
                controller.sort_by_label(&labels, id == "view.tabs.sort_descending");
                if id == "view.tabs.sort_path" {
                    controller.tab_sort = "path".into();
                }
            }
            "view.tabs.move_left" | "view.tabs.move_right" => {
                if let Some(id_active) = active {
                    let _ = controller.keyboard_reorder(id_active, id == "view.tabs.move_left");
                }
            }
            "view.tabs.previous" | "view.tabs.next" => {
                let tabs: Vec<_> = controller
                    .pane_tabs(controller.active_pane())
                    .map(|tab| tab.id)
                    .collect();
                if let Some(index) = active.and_then(|id| tabs.iter().position(|tab| *tab == id)) {
                    let next = if id == "view.tabs.previous" {
                        (index + tabs.len() - 1) % tabs.len()
                    } else {
                        (index + 1) % tabs.len()
                    };
                    let id = tabs[next];
                    self.views.select_tab(workspace, &mut self.app, id);
                }
            }
            "view.tabs.mru" => {
                if let Some(popup) = &mut self.views.mru_popup {
                    if !popup.ids.is_empty() {
                        popup.selected = (popup.selected + 1) % popup.ids.len();
                    }
                } else {
                    let mut seen = std::collections::HashSet::new();
                    let ids = controller
                        .mru()
                        .filter(|id| {
                            controller
                                .tab(*id)
                                .is_some_and(|tab| seen.insert(tab.document_id))
                        })
                        .collect::<Vec<_>>();
                    let selected = usize::from(ids.len() > 1);
                    self.views.mru_popup = Some(MruPopup {
                        ids,
                        selected,
                        bounds: Rect::default(),
                    });
                }
            }
            _ => return false,
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    fn tabs_event(&mut self, el: &ActiveEventLoop, event: &WindowEvent) -> bool {
        if self.palette.open {
            return false;
        }
        let origin = self.editor_bounds();
        let point = Point {
            x: self.pointer.x - origin.x,
            y: self.pointer.y - origin.y,
        };
        let mut handled = false;
        let Some(workspace) = &mut self.workspace else {
            return false;
        };
        if self.views.mru_popup.is_some() {
            let mut accept = false;
            let mut cancel = false;
            let popup = self.views.mru_popup.as_mut().unwrap();
            match event {
                WindowEvent::ModifiersChanged(modifiers) if !modifiers.state().control_key() => {
                    accept = true
                }
                WindowEvent::KeyboardInput { event, .. }
                    if event.state == ElementState::Pressed =>
                {
                    match event.logical_key {
                        Key::Named(NamedKey::Escape) => cancel = true,
                        Key::Named(NamedKey::Enter) => accept = true,
                        Key::Named(NamedKey::ArrowUp) => {
                            popup.selected = popup.selected.saturating_sub(1);
                        }
                        Key::Named(NamedKey::ArrowDown) | Key::Named(NamedKey::Tab) => {
                            if !popup.ids.is_empty() {
                                popup.selected = (popup.selected + 1) % popup.ids.len();
                            }
                        }
                        _ => {}
                    }
                }
                WindowEvent::MouseInput {
                    state: ElementState::Pressed,
                    button: MouseButton::Left,
                    ..
                } => {
                    if popup.bounds.contains(point) {
                        let start = popup
                            .selected
                            .saturating_sub(mru_visible_rows(popup.bounds).saturating_sub(1));
                        popup.selected = (start
                            + ((point.y - popup.bounds.y) / TAB_HEIGHT) as usize)
                            .min(popup.ids.len().saturating_sub(1));
                        accept = true;
                    } else {
                        cancel = true;
                    }
                }
                _ => return false,
            }
            if accept || cancel {
                let popup = self.views.mru_popup.take().unwrap();
                if accept {
                    if let Some(id) = popup.ids.get(popup.selected) {
                        self.views.select_tab(workspace, &mut self.app, *id);
                    }
                }
            }
            handled = true;
        } else {
            match event {
                WindowEvent::MouseInput {
                    state: ElementState::Pressed,
                    button: MouseButton::Left,
                    ..
                } => {
                    if let Some((pane, next, _)) = self
                        .views
                        .tab_nav
                        .iter()
                        .find(|(_, _, bounds)| bounds.contains(point))
                        .copied()
                    {
                        let offset = &mut self.views.tab_offset[pane as usize];
                        *offset = if next {
                            offset.saturating_add(1)
                        } else {
                            offset.saturating_sub(1)
                        };
                        handled = true;
                    } else if let Some(hit) = self
                        .views
                        .tab_hits
                        .iter()
                        .find(|hit| hit.bounds.contains(point))
                        .copied()
                    {
                        if hit.close.contains(point) {
                            self.views.close_tab(workspace, &mut self.app, hit.id);
                        } else {
                            self.views.select_tab(workspace, &mut self.app, hit.id);
                            self.views.tab_drag = Some(TabDrag {
                                id: hit.id,
                                start: point,
                                moved: false,
                            });
                        }
                        handled = true;
                    }
                }
                WindowEvent::CursorMoved { position, .. } if self.views.tab_drag.is_some() => {
                    let scale = self
                        .window
                        .as_ref()
                        .map_or(1.0, |window| window.scale_factor());
                    let p = position.to_logical::<f32>(scale);
                    self.pointer = Point { x: p.x, y: p.y };
                    let point = Point {
                        x: p.x - origin.x,
                        y: p.y - origin.y,
                    };
                    let drag = self.views.tab_drag.as_mut().unwrap();
                    drag.moved |=
                        (point.x - drag.start.x).abs() + (point.y - drag.start.y).abs() > 5.0;
                    handled = true;
                }
                WindowEvent::MouseInput {
                    state: ElementState::Released,
                    button: MouseButton::Left,
                    ..
                } if self.views.tab_drag.is_some() => {
                    let drag = self.views.tab_drag.take().unwrap();
                    if drag.moved && !self.views.busy(workspace) {
                        let target = self
                            .views
                            .tab_hits
                            .iter()
                            .find(|hit| hit.bounds.contains(point))
                            .map(|hit| (hit.pane, Some(hit.id)))
                            .or_else(|| {
                                self.views
                                    .bounds
                                    .iter()
                                    .enumerate()
                                    .find(|(_, bounds)| {
                                        bounds.is_some_and(|bounds| bounds.contains(point))
                                    })
                                    .map(|(pane, _)| (pane as u32, None))
                            });
                        if let Some((pane, before)) = target {
                            self.views.save_current(workspace);
                            if let Some(controller) = &mut self.views.controller {
                                if let Err(error) = controller.move_to_pane(drag.id, pane, before) {
                                    workspace.message =
                                        Some(format!("Tab cannot be moved: {error:?}"));
                                }
                            }
                            self.views.loaded_tabs = [None, None];
                            self.views.install_views(workspace);
                            self.views.select_tab(workspace, &mut self.app, drag.id);
                        }
                    }
                    handled = true;
                }
                WindowEvent::MouseWheel { delta, .. }
                    if self
                        .views
                        .tab_strips
                        .iter()
                        .any(|bounds| bounds.is_some_and(|bounds| bounds.contains(point))) =>
                {
                    let pane = self
                        .views
                        .tab_strips
                        .iter()
                        .position(|bounds| bounds.is_some_and(|bounds| bounds.contains(point)))
                        .unwrap();
                    let down = match delta {
                        MouseScrollDelta::LineDelta(_, y) => *y < 0.0,
                        MouseScrollDelta::PixelDelta(point) => point.y < 0.0,
                    };
                    let offset = &mut self.views.tab_offset[pane];
                    *offset = if down {
                        offset.saturating_add(1)
                    } else {
                        offset.saturating_sub(1)
                    };
                    handled = true;
                }
                WindowEvent::Focused(false) => {
                    self.views.tab_drag = None;
                }
                _ => {}
            }
        }
        let close = self.views.pending_close.take();
        if close.is_some() {
            self.dispatch(el, Action::Close);
        }
        if handled {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        handled
    }
    pub(super) fn views_dispatch(&mut self, _el: &ActiveEventLoop, id: &str) -> bool {
        if self.tabs_dispatch(id) {
            return true;
        }
        if !matches!(
            id,
            "view.split_vertical"
                | "view.split_horizontal"
                | "view.clone_other"
                | "view.move_other"
                | "view.close_split"
                | "view.focus_other"
                | "view.sync_vertical"
                | "view.sync_horizontal"
        ) {
            return false;
        }
        let Some(workspace) = &mut self.workspace else {
            return true;
        };
        self.views.pump(workspace);
        match id {
            "view.split_vertical" => {
                self.views
                    .split(workspace, self.app.active, Orientation::Vertical)
            }
            "view.clone_other" => self.views.clone_active(workspace, &mut self.app),
            "view.move_other" => self.views.move_active(workspace, &mut self.app),
            "view.split_horizontal" => {
                self.views
                    .split(workspace, self.app.active, Orientation::Horizontal)
            }
            "view.close_split" if !self.views.busy(workspace) => {
                self.views.collapse(workspace, self.views.pane() == 1)
            }
            "view.focus_other" if self.views.open() => {
                let pane = 1 - self.views.pane();
                self.views.activate(workspace, &mut self.app, pane);
            }
            "view.sync_vertical" => {
                if let Some(controller) = &mut self.views.controller {
                    controller.sync_vertical = !controller.sync_vertical;
                }
            }
            "view.sync_horizontal" => {
                if let Some(controller) = &mut self.views.controller {
                    controller.sync_horizontal = !controller.sync_horizontal;
                }
            }
            _ => {}
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    pub(super) fn views_action(&mut self, _el: &ActiveEventLoop, action: Action) -> bool {
        if !self.views.open() && action != Action::Close {
            return false;
        }
        let Some(workspace) = &mut self.workspace else {
            return false;
        };
        self.views.pump(workspace);
        if matches!(
            action,
            Action::Save | Action::SaveAs | Action::Close | Action::Quit
        ) && self.views.busy(workspace)
        {
            workspace.message =
                Some("Wait for pending split-view edits before saving or closing.".into());
            return true;
        }
        if action == Action::Close {
            self.views.save_current(workspace);
            if let Some(id) = self
                .views
                .controller
                .as_ref()
                .and_then(|controller| controller.active_tab(controller.active_pane()))
            {
                let controller = self.views.controller.as_ref().unwrap();
                let document = controller.tab(id).unwrap().document_id;
                if controller
                    .tabs()
                    .iter()
                    .filter(|tab| tab.document_id == document)
                    .count()
                    > 1
                {
                    self.views.close_tab(workspace, &mut self.app, id);
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                    return true;
                }
            }
            return false;
        }
        let pane = self.views.pane();
        let input = match action {
            Action::Undo => Some(Input::Undo),
            Action::Redo => Some(Input::Redo),
            Action::SelectAll => Some(Input::SelectAll),
            Action::Paste => self
                .platform
                .as_ref()
                .and_then(|p| p.clipboard_text().ok())
                .map(Input::Insert),
            Action::Copy | Action::Cut => {
                let editor = if pane == 1 {
                    self.views.secondary.as_ref()
                } else {
                    self.views
                        .primary_index(workspace)
                        .and_then(|i| workspace.editors.get(i))
                };
                if matches!(editor, Some(WorkspaceEditor::Paged(paged)) if !paged.selection_fully_in_viewport()) {
                    workspace.message = Some("Reveal the complete selection before copying or cutting it.".into());
                    return true;
                }
                let copied = editor
                    .and_then(|e| e.selected_text().ok())
                    .is_some_and(|value| {
                        !value.is_empty()
                            && self
                                .platform
                                .as_ref()
                                .is_some_and(|p| p.set_clipboard_text(&value).is_ok())
                    });
                if copied && action == Action::Cut {
                    Some(Input::Insert(String::new()))
                } else {
                    None
                }
            }
            _ => return false,
        };
        if let Some(input) = input {
            self.views.input(workspace, pane, input);
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
    pub(super) fn views_event(&mut self, _el: &ActiveEventLoop, event: &WindowEvent) -> bool {
        if matches!(
            event,
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                ..
            } | WindowEvent::KeyboardInput { .. }
        ) {
            self.views.accessibility_focus = None;
        }
        if self.tabs_event(_el, event) {
            return true;
        }
        if (!self.views.open()
            && !self
                .views
                .controller
                .as_ref()
                .is_some_and(|controller| controller.vertical_tabs))
            || self.palette.open
        {
            return false;
        }
        let editor_bounds = self.editor_bounds();
        let pointer = Point {
            x: self.pointer.x - editor_bounds.x,
            y: self.pointer.y - editor_bounds.y,
        };
        let Some(workspace) = &mut self.workspace else {
            return false;
        };
        if workspace.find.has_focus() || workspace.search_focus {
            return false;
        }
        let Some(window) = &self.window else {
            return false;
        };
        let mut handled = false;
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                let p = position.to_logical::<f32>(window.scale_factor());
                let p = Point {
                    x: p.x - editor_bounds.x,
                    y: p.y - editor_bounds.y,
                };
                if self.views.dragging {
                    if let (Some(first), Some(second), Some(controller)) = (
                        self.views.bounds[0],
                        self.views.bounds[1],
                        &mut self.views.controller,
                    ) {
                        controller.ratio = if controller.orientation == Orientation::Vertical {
                            ((p.x - first.x) / (second.x + second.width - first.x)).clamp(0.1, 0.9)
                                as f64
                        } else {
                            ((p.y - first.y) / (second.y + second.height - first.y)).clamp(0.1, 0.9)
                                as f64
                        };
                    }
                    handled = true;
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if self.views.splitter.is_some_and(|r| r.contains(pointer)) {
                    self.views.dragging = true;
                    handled = true;
                } else if let Some(pane) = self
                    .views
                    .bounds
                    .iter()
                    .position(|r| r.is_some_and(|r| r.contains(pointer)))
                {
                    self.views.activate(workspace, &mut self.app, pane as u32);
                    let bounds = self.views.bounds[pane].unwrap();
                    let editor = if pane == 1 {
                        self.views.secondary.as_mut()
                    } else {
                        self.views
                            .primary_index(workspace)
                            .and_then(|i| workspace.editors.get_mut(i))
                            
                    };
                    if let (Some(editor), Some(renderer)) = (editor, &self.renderer) {
                        if matches!(&*editor, WorkspaceEditor::Paged(paged) if !paged.paged_frame_state().ready) { return true; }
                        let _ = editor.click(
                            renderer,
                            Point {
                                x: pointer.x - bounds.x,
                                y: pointer.y - bounds.y,
                            },
                            self.modifiers.shift_key(),
                        );
                    }
                    handled = true;
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } if self.views.dragging => {
                self.views.dragging = false;
                handled = true;
            }
            WindowEvent::Focused(false) => {
                self.views.dragging = false;
                if let Some(peer) = &mut self.views.secondary {
                    peer.cancel_composition();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if let Some(pane) = self
                    .views
                    .bounds
                    .iter()
                    .position(|r| r.is_some_and(|r| r.contains(pointer)))
                {
                    if self.modifiers.control_key() {
                        let steps = match delta {
                            MouseScrollDelta::LineDelta(_, y) => *y,
                            MouseScrollDelta::PixelDelta(point) => {
                                (point.y / window.scale_factor() / 48.0) as f32
                            }
                        };
                        let index = self.views.primary_index(workspace);
                        let editor = if pane == 1 {
                            self.views.secondary.as_mut()
                        } else {
                            index.and_then(|index| workspace.editors.get_mut(index))
                        };
                        if let Some(editor) = editor {
                            editor.zoom_by(steps);
                        }
                        window.request_redraw();
                        return true;
                    }
                    let horizontal = self.modifiers.shift_key()
                        || matches!(delta,MouseScrollDelta::LineDelta(x,y) if x.abs()>y.abs())
                        || matches!(delta,MouseScrollDelta::PixelDelta(point) if point.x.abs()>point.y.abs());
                    let amount = match delta {
                        MouseScrollDelta::LineDelta(x, _)
                            if horizontal && !self.modifiers.shift_key() =>
                        {
                            -*x as f64 * 72.0
                        }
                        MouseScrollDelta::PixelDelta(p)
                            if horizontal && !self.modifiers.shift_key() =>
                        {
                            -p.x / window.scale_factor()
                        }
                        MouseScrollDelta::LineDelta(_, y) => -*y as f64 * 72.0,
                        MouseScrollDelta::PixelDelta(p) => -p.y / window.scale_factor(),
                    };
                    let height = self.views.bounds[pane].unwrap().height + 24.0;
                    if pane == 1 {
                        if let Some(peer) = &mut self.views.secondary {
                            if horizontal {
                                peer.scroll_horizontal(amount);
                            } else {
                                if amount < 0.0 {
                                    if let WorkspaceEditor::Paged(editor) = peer {
                                        editor.set_follow_paused(true);
                                    }
                                }
                                scroll_workspace_view(peer, amount, height);
                            }
                        }
                    } else if let Some(index) = self.views.primary_index(workspace) {
                        if horizontal {
                            workspace.editors[index].scroll_horizontal(amount);
                        } else {
                            if amount < 0.0 {
                                if let WorkspaceEditor::Paged(editor) =
                                    &mut workspace.editors[index]
                                {
                                    editor.set_follow_paused(true);
                                }
                            }
                            scroll_workspace_view(&mut workspace.editors[index], amount, height);
                        }
                    }
                    self.views.sync_scroll(workspace, pane as u32);
                    handled = true;
                }
            }
            WindowEvent::Ime(ime) => {
                let pane = self.views.pane();
                let editor = if pane == 1 {
                    self.views.secondary.as_mut().map(|editor| &mut **editor)
                } else {
                    self.views
                        .primary_index(workspace)
                        .and_then(|i| workspace.editors.get_mut(i))
                        .map(|editor| &mut **editor)
                };
                if let Some(peer) = editor {
                    match ime {
                        Ime::Preedit(value, cursor) => peer.preedit(value.clone(), *cursor),
                        Ime::Commit(_) => peer.cancel_composition(),
                        Ime::Disabled => peer.cancel_composition(),
                        Ime::Enabled => {}
                    }
                }
                if let Ime::Commit(value) = ime {
                    self.views
                        .input(workspace, pane, Input::Insert(value.clone()));
                }
                handled = true;
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let extend = self.modifiers.shift_key();
                if matches!(
                    event.logical_key,
                    Key::Named(NamedKey::PageDown | NamedKey::PageUp)
                ) && !self.modifiers.control_key()
                {
                    let forward = matches!(event.logical_key, Key::Named(NamedKey::PageDown));
                    let pane = self.views.pane();
                    let height =
                        self.views.bounds[pane as usize].map_or(400.0, |bounds| bounds.height);
                    let index = self.views.primary_index(workspace);
                    let editor = if pane == 1 {
                        self.views.secondary.as_mut()
                    } else {
                        index.and_then(|index| workspace.editors.get_mut(index))
                    };
                    if let Some(editor) = editor {
                        if !editor.busy() {
                            if !forward {
                                if let WorkspaceEditor::Paged(paged) = editor {
                                    paged.set_follow_paused(true);
                                }
                            }
                            scroll_workspace_view(
                                editor,
                                if forward {
                                    height as f64 * 0.8
                                } else {
                                    -height as f64 * 0.8
                                },
                                height,
                            );
                        }
                    }
                    self.views.sync_scroll(workspace, pane);
                    window.request_redraw();
                    return true;
                }
                let input = match &event.logical_key {
                    Key::Named(NamedKey::ArrowLeft) => Some(Input::Left(extend)),
                    Key::Named(NamedKey::ArrowRight) => Some(Input::Right(extend)),
                    Key::Named(NamedKey::ArrowUp) => Some(Input::Up(extend)),
                    Key::Named(NamedKey::ArrowDown) => Some(Input::Down(extend)),
                    Key::Named(NamedKey::Home) => Some(Input::Home(extend)),
                    Key::Named(NamedKey::End) => Some(Input::End(extend)),
                    Key::Named(NamedKey::Backspace) => Some(Input::Backspace),
                    Key::Named(NamedKey::Delete) => Some(Input::Delete),
                    Key::Named(NamedKey::Enter) => Some(Input::Insert("\n".into())),
                    Key::Named(NamedKey::Tab) if !self.modifiers.control_key() => {
                        Some(Input::Insert("\t".into()))
                    }
                    _ if !self.modifiers.control_key() || self.modifiers.alt_key() => event
                        .text
                        .as_ref()
                        .filter(|text| !text.is_empty() && !text.chars().any(|c| c.is_control()))
                        .map(|text| Input::Insert(text.to_string())),
                    _ => None,
                };
                if let Some(input) = input {
                    let pane = self.views.pane();
                    self.views.input(workspace, pane, input);
                    handled = true;
                }
            }
            _ => {}
        }
        if handled {
            window.request_redraw();
        }
        handled
    }
}
