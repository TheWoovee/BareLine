// SPDX-License-Identifier: MPL-2.0
//! Native two-pane consumer. Both surfaces address the same document actor when cloned.
use super::*;
use bareline_app::views::{
    Orientation, ScrollPosition, SessionTab, SharedEditorView, ViewController, ViewSnapshot, ViewState,
};
use bareline_app::workspace::WorkspaceEditor;
use bareline_commands::{CommandId, CommandPresentation, CommandRegistry, CommandSpec};
use bareline_renderer::{DrawOp, LayoutError, Rect, TextBackend};
use bareline_ui::{ACCENT, BORDER, CHROME, MUTED, TAB_HEIGHT, TEXT, rect, text};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, VecDeque},
};

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
            "open"
                | "populated"
                | "focus_close"
                | "focus_overflow"
                | "mru"
                | "vertical"
                | "split_vertical"
                | "split_horizontal"
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
    if scenario.starts_with("split_") {
        let orientation = if scenario == "split_horizontal" {
            Orientation::Horizontal
        } else {
            Orientation::Vertical
        };
        shell.views.split(&mut workspace, 0, orientation);
        shell.views.bounds = if orientation == Orientation::Vertical {
            [
                Some(rect(0.0, 24.0, 496.0, 752.0)),
                Some(rect(504.0, 24.0, 496.0, 752.0)),
            ]
        } else {
            [
                Some(rect(0.0, 24.0, 1000.0, 372.0)),
                Some(rect(0.0, 404.0, 1000.0, 372.0)),
            ]
        };
        shell.views.activate(&mut workspace, &mut shell.app, 1);
    }
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
    shell.views.draw_mru(workspace, 1000.0, 800.0, &mut operations);
    assert!(!operations.is_empty(), "fixture must retain production layout");
    if scenario == "focus_close" {
        let hit = shell.views.tab_hits.iter().find(|hit| hit.id == first).unwrap();
        shell.views.accessibility_focus = access_tab_id(hit.id).map(|id| id + 1);
    } else if scenario == "focus_overflow" {
        let (pane, forward, _) = shell.views.tab_nav.first().unwrap();
        shell.views.accessibility_focus = Some(ACCESS_NAV_BASE + *pane as u64 * 2 + u64::from(*forward));
    }
}

#[cfg(test)]
pub(super) fn accessibility_test_close_split(shell: &mut Shell) -> bool {
    let Some(workspace) = shell.workspace.as_mut() else {
        return false;
    };
    shell.views.pump(workspace);
    shell.views.close_split(workspace);
    !shell.views.open()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_multiple_targets_follow_the_strip_and_spare_pinned_tabs() {
        // Strip order 1..=5 with tab 2 pinned and tab 3 active.
        let strip = [(1, false), (2, true), (3, false), (4, false), (5, false)];
        let close = |id| close_targets(&strip, Some(3), id);
        assert_eq!(close("view.tabs.closeAll"), vec![1, 2, 3, 4, 5]);
        assert_eq!(close("view.tabs.closeOthers"), vec![1, 4, 5]);
        assert_eq!(close("view.tabs.closeLeft"), vec![1]);
        assert_eq!(close("view.tabs.closeRight"), vec![4, 5]);
        assert!(close("view.tabs.unknown").is_empty());
        // At the strip edges there is nothing to the left or right.
        assert!(close_targets(&strip, Some(1), "view.tabs.closeLeft").is_empty());
        assert!(close_targets(&strip, Some(5), "view.tabs.closeRight").is_empty());
        // Without an active tab only Close All applies.
        assert!(close_targets(&strip, None, "view.tabs.closeOthers").is_empty());
        assert_eq!(close_targets(&strip, None, "view.tabs.closeAll").len(), 5);
    }

    #[test]
    fn close_multiple_closes_the_strip_one_tab_at_a_time() {
        let mut workspace = Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        for _ in 0..3 {
            workspace.new_document().unwrap();
        }
        let mut views = ViewsRuntime::default();
        views.sync_documents(&workspace);
        let controller = views.controller.as_ref().unwrap();
        let pane = controller.active_pane();
        let strip: Vec<(u64, bool)> = controller.pane_tabs(pane).map(|tab| (tab.id, tab.pinned)).collect();
        assert_eq!(strip.len(), 3);
        let mut context = bareline_commands::CommandContext::default();
        views.annotate_context(&mut context, &workspace.titles(), 0);
        for id in CLOSE_MULTIPLE_IDS {
            let disabled = context.states.get(&CommandId(id)).is_some_and(|state| !state.enabled);
            let expected = close_targets(&strip, controller.active_tab(pane), id).is_empty();
            assert_eq!(disabled, expected, "{id}");
        }
        assert!(
            context
                .states
                .get(&CommandId("view.tabs.closeAll"))
                .is_none_or(|state| state.enabled)
        );
        // While a batch runs, starting another is refused.
        views.close_queue.push_back(strip[0].0);
        let mut busy = bareline_commands::CommandContext::default();
        views.annotate_context(&mut busy, &workspace.titles(), 0);
        for id in CLOSE_MULTIPLE_IDS {
            assert!(
                busy.states.get(&CommandId(id)).is_some_and(|state| !state.enabled),
                "{id}"
            );
        }
    }

    #[test]
    fn vertical_tabs_share_find_draw_and_hover_geometry() {
        let mut workspace = Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.new_document().unwrap();
        workspace.find.show();
        let mut views = ViewsRuntime::default();
        views.sync_documents(&workspace);
        views.controller.as_mut().unwrap().vertical_tabs = true;

        let (inset, find_width) = views.find_horizontal_geometry(1000.0);
        assert_eq!((inset, find_width), (176.0, 824.0));
        let window_point = Point {
            x: inset + find_width - 458.0 + 2.0,
            y: TAB_HEIGHT + 14.0,
        };
        assert!(workspace.find.hover_toggles(
            find_width,
            Point {
                x: window_point.x - inset,
                y: window_point.y,
            },
            100,
        ));
        assert_eq!(workspace.find.tooltip_deadline_ms(), Some(600));
    }

    #[test]
    fn find_selection_scope_uses_the_active_split_view() {
        let mut workspace = Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.new_document().unwrap();
        let mut views = ViewsRuntime::default();
        views.sync_documents(&workspace);
        views.split(&mut workspace, 0, Orientation::Vertical);
        workspace.editors[0].viewport_mut().selection.anchor = 1;
        workspace.editors[0].viewport_mut().selection.caret = 2;
        let secondary = views.secondary.as_mut().unwrap();
        secondary.viewport_mut().selection.anchor = 7;
        secondary.viewport_mut().selection.caret = 4;

        let mut app = App::default();
        views.activate(&mut workspace, &mut app, 1);
        assert_eq!(
            views.active_selection(&workspace, app.active),
            Some(bareline_document::TextOffset(4)..bareline_document::TextOffset(7))
        );
        views.activate(&mut workspace, &mut app, 0);
        assert_eq!(
            views.active_selection(&workspace, app.active),
            Some(bareline_document::TextOffset(1)..bareline_document::TextOffset(2))
        );
    }

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
        let path = std::env::temp_dir().join(format!("bareline-mapped-native-{}.txt", std::process::id()));
        let body = format!(
            "header\n{}suffix\n",
            "interior row with bounded content\n".repeat(12_000)
        );
        let suffix = body.find("suffix").unwrap();
        assert!(suffix > 256 * 1024);
        std::fs::write(&path, &body).unwrap();
        let mut workspace = Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.resident_max_bytes = 1;
        workspace.open(path.clone());
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            assert!(Instant::now() < deadline);
            workspace.pump();
            if workspace
                .editors
                .first()
                .is_some_and(|editor| matches!(editor,WorkspaceEditor::Paged(p) if p.viewport_ready()))
            {
                break;
            }
            std::thread::yield_now();
        }
        let mut views = ViewsRuntime::default();
        views.split(&mut workspace, 0, Orientation::Vertical);
        loop {
            assert!(Instant::now() < deadline);
            workspace.pump();
            views.pump(&mut workspace);
            if !views.busy(&workspace)
                && views.pending_restore.iter().all(Option::is_none)
                && views.pending_view_scroll.iter().all(Option::is_none)
            {
                break;
            }
            std::thread::yield_now();
        }
        let WorkspaceEditor::Paged(peer) = views.secondary.as_mut().unwrap() else {
            unreachable!()
        };
        peer.set_known_global_folds(
            vec![bareline_syntax::folding::Fold {
                header: 0,
                end: 12_000,
                level: 1,
            }],
            1,
            false,
            0,
        )
        .unwrap();
        peer.fold_all_known(1);
        assert_eq!(peer.persisted_global_folds(), vec![0..12_001]);
        loop {
            assert!(Instant::now() < deadline);
            peer.pump();
            if peer.paged_frame_state().ready && peer.source_segments().len() > 1 {
                break;
            }
            std::thread::yield_now();
        }
        assert!(peer.local_offset(TextOffset(100_000)).is_none());
        let suffix_local = peer.local_offset(TextOffset(suffix)).unwrap();
        assert_eq!(
            peer.source_offset(suffix_local, SourceAffinity::After),
            Some(TextOffset(suffix))
        );
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        let mut ops = Vec::new();
        peer.viewport_mut()
            .draw_styled(
                &mut renderer,
                1000.0,
                800.0,
                &mut ops,
                bareline_editor_surface::SyntaxView {
                    result: None,
                    language: "Plain text",
                    unavailable: false,
                },
            )
            .unwrap();
        let suffix_box = peer
            .viewport()
            .accessibility_geometry(&renderer, 1000.0, 800.0)
            .into_iter()
            .find(|(range, _)| range.start == suffix_local.0)
            .unwrap()
            .1;
        let mut boxes = peer
            .viewport()
            .accessibility_geometry(&renderer, 1000.0, 800.0)
            .into_iter()
            .map(
                |(range, bounds)| bareline_platform::accessibility::AccessibilityTextBox {
                    start: range.start,
                    end: range.end,
                    bounds: [
                        bounds.x as f64,
                        bounds.y as f64,
                        bounds.width as f64,
                        bounds.height as f64,
                    ],
                },
            )
            .collect();
        super::super::accessibility::map_paged_geometry(peer, &mut boxes);
        let footer = boxes
            .iter()
            .find(|rect| rect.bounds[0] == suffix_box.x as f64 && rect.bounds[1] == suffix_box.y as f64)
            .unwrap();
        assert_eq!(footer.start, suffix);
        assert!(boxes.iter().any(|rect| rect.start == 0));
        assert!(boxes.iter().all(|rect| rect.end <= 7 || rect.start >= suffix));
        let sources = [
            bareline_app::accessibility::text_source(&workspace.editors[0], std::sync::Arc::new(|| {})),
            bareline_app::accessibility::text_source(views.secondary.as_ref().unwrap(), std::sync::Arc::new(|| {})),
        ];
        for source in &sources {
            assert_eq!(source.len(), body.len());
            loop {
                match source.read(suffix, 7) {
                    bareline_platform::accessibility::AccessibleRead::Ready { start, text } => {
                        assert_eq!(start, suffix);
                        assert_eq!(text, "suffix\n");
                        break;
                    }
                    bareline_platform::accessibility::AccessibleRead::Pending => {
                        assert!(Instant::now() < deadline);
                        std::thread::yield_now();
                    }
                    _ => panic!("Visible footer unavailable through pane text reader"),
                }
            }
        }
        let provider_before = views
            .accessibility_source_identity(1, views.secondary.as_ref().unwrap())
            .unwrap();
        workspace.find.show_replace();
        workspace.find.field.insert("suffix");
        let search_handle = match views.secondary.as_ref().unwrap() {
            WorkspaceEditor::Paged(editor) => editor.read_handle(),
            _ => unreachable!(),
        };
        workspace.find.refresh_paged(search_handle, std::sync::Arc::new(|| {}));
        let search_deadline = Instant::now() + Duration::from_secs(30);
        while workspace.find.searching() {
            assert!(Instant::now() < search_deadline);
            workspace.find.pump();
            std::thread::yield_now();
        }
        assert_eq!(workspace.find.completed_paged_results().unwrap().count, 1);
        assert!(
            !workspace
                .find
                .semantics(1000.0)
                .into_iter()
                .find(|node| node.name == "Replace All")
                .unwrap()
                .disabled
        );
        assert_eq!(views.pane_document_index(&workspace, 0), Some(0));
        assert_eq!(views.pane_document_index(&workspace, 1), Some(0));
        let announced = super::super::accessibility::editor_provider_name(&workspace, 1, 0);
        assert!(announced.contains(path.file_name().unwrap().to_string_lossy().as_ref()));
        assert!(announced.contains(path.display().to_string().as_str()));
        let WorkspaceEditor::Paged(primary) = &workspace.editors[0] else {
            unreachable!()
        };
        let primary_selection = primary.global_selection();
        let primary_viewport = primary.viewport_start();
        let WorkspaceEditor::Paged(peer) = views.secondary.as_mut().unwrap() else {
            unreachable!()
        };
        peer.click(
            &renderer,
            Point {
                x: suffix_box.x + 0.1,
                y: suffix_box.y + suffix_box.height * 0.5,
            },
            false,
        )
        .unwrap();
        while peer.busy() {
            assert!(Instant::now() < deadline);
            peer.pump();
            std::thread::yield_now();
        }
        assert_eq!(peer.global_selection().1.0, suffix);
        let WorkspaceEditor::Paged(primary) = &workspace.editors[0] else {
            unreachable!()
        };
        assert_eq!(primary.global_selection(), primary_selection);
        assert_eq!(primary.viewport_start(), primary_viewport);
        let prior = peer.global_selection();
        peer.request_viewport(TextOffset(0)).unwrap();
        peer.click(
            &renderer,
            Point {
                x: suffix_box.x + 0.1,
                y: suffix_box.y + suffix_box.height * 0.5,
            },
            false,
        )
        .unwrap();
        assert_eq!(peer.global_selection(), prior);
        while peer.busy() {
            assert!(Instant::now() < deadline);
            peer.pump();
            std::thread::yield_now();
        }
        assert_eq!(
            views
                .accessibility_source_identity(1, views.secondary.as_ref().unwrap())
                .unwrap(),
            provider_before,
            "viewport movement must retain the pane source generation"
        );
        let viewport_revision = views.secondary.as_ref().unwrap().viewport().snapshot().revision;
        views.secondary.as_mut().unwrap().enqueue(Input::Insert("x".into()));
        while views.busy(&workspace) {
            assert!(Instant::now() < deadline);
            workspace.pump();
            views.pump(&mut workspace);
            std::thread::yield_now();
        }
        assert_ne!(
            views
                .accessibility_source_identity(1, views.secondary.as_ref().unwrap())
                .unwrap(),
            provider_before,
            "a full paged document edit must retire the old pane reader"
        );
        let changed_source = match views.secondary.as_ref().unwrap() {
            WorkspaceEditor::Paged(editor) => editor.snapshot().clone(),
            _ => unreachable!(),
        };
        workspace.bind_find_paged(&changed_source);
        assert_eq!(workspace.find.status, "Results changed; search again");
        assert!(workspace.find.completed_paged_results().is_none());
        assert!(
            workspace
                .find
                .semantics(1000.0)
                .into_iter()
                .find(|node| node.name == "Replace All")
                .unwrap()
                .disabled
        );
        assert_eq!(
            views.secondary.as_ref().unwrap().viewport().snapshot().revision,
            viewport_revision,
            "the bounded viewport revision is deliberately reused and cannot qualify the full source"
        );
        let WorkspaceEditor::Paged(primary) = &workspace.editors[0] else {
            unreachable!()
        };
        assert!(primary.source_segments().is_empty());
        assert_eq!(primary.global_selection(), primary_selection);
        assert_eq!(primary.viewport_start(), primary_viewport);
        drop(views);
        drop(workspace);
        let _ = std::fs::remove_file(path);
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
            assert!(Instant::now() < deadline, "global synchronized navigation timed out");
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
        let selection_token = target
            .restore_global_selection(bareline_document::TextOffset(2), bareline_document::TextOffset(9), true)
            .unwrap();
        loop {
            target.pump();
            match target.selection_restore_status(selection_token) {
                bareline_editor_surface::paged_view::SelectionRestoreStatus::Pending => {
                    assert!(Instant::now() < deadline);
                    std::thread::yield_now();
                }
                bareline_editor_surface::paged_view::SelectionRestoreStatus::Applied => break,
                status => panic!("Global selection failed: {status:?}"),
            }
        }
        assert_eq!(target.viewport_start(), original_viewport);
        assert_eq!(
            target.global_selection(),
            (bareline_document::TextOffset(2), bareline_document::TextOffset(9))
        );
        let mut peer = target.clone_view().unwrap();
        peer.pump();
        assert_eq!(peer.global_selection(), target.global_selection());
        target.viewport_mut().scroll_y = 17.5;
        target.restore_global_folds(&[8004..8011]);
        let saved_byte = target.viewport_start();
        target.request_viewport(saved_byte).unwrap();
        assert_eq!(target.global_logical_scroll(), GlobalScrollPosition::Pending);
        let mut pending_ops = Vec::new();
        assert!(bareline_app::workspace::paint_paged_pending(
            views.secondary.as_ref().unwrap(),
            1000.0,
            800.0,
            workspace.theme,
            &mut pending_ops
        ));
        assert!(!pending_ops.iter().any(|op| matches!(op, DrawOp::Layout { .. })));
        let expected = workspace_view_state(views.secondary.as_ref().unwrap());
        assert_eq!(expected.scroll_byte, Some(saved_byte.0 as u64));
        assert_eq!(expected.folds, vec![8004..8011]);
        let provider_ids =
            [0, 1].map(|pane| super::super::accessibility::editor_provider_id(views.pane_token(pane).unwrap()));
        let provider_sources = [
            views
                .accessibility_source_identity(0, views.pane_workspace_editor(&workspace, 0, 0).unwrap())
                .unwrap(),
            views
                .accessibility_source_identity(1, views.pane_workspace_editor(&workspace, 0, 1).unwrap())
                .unwrap(),
        ];
        assert_ne!(provider_ids[0], provider_ids[1]);
        assert_ne!(provider_sources[0], provider_sources[1]);
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
        let manifest =
            bareline_file_io::session::decode(&bareline_file_io::session::encode(&manifest).unwrap()).unwrap();
        let mut restored = ViewsRuntime::default();
        let mut app = App::default();
        restored.restore_session(&mut workspace, &mut app, &manifest, &[(1, 0), (2, 0)]);
        loop {
            assert!(Instant::now() < deadline, "byte anchored view restoration timed out");
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
            [0, 1].map(|pane| super::super::accessibility::editor_provider_id(restored.pane_token(pane).unwrap())),
            provider_ids,
            "session restore must retain both paged provider identities"
        );
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
        let before = editor.viewport().selection;
        let mut invalid = workspace_view_state(editor);
        let WorkspaceEditor::Paged(paged) = &mut *editor else {
            unreachable!()
        };
        let text = paged
            .viewport()
            .snapshot()
            .read(
                bareline_document::TextOffset(0)..bareline_document::TextOffset(paged.viewport().snapshot().len()),
                64 * 1024,
            )
            .unwrap();
        invalid.anchor = paged
            .source_offset(
                bareline_document::TextOffset(text.find('é').unwrap() + 1),
                bareline_editor_surface::paged_view::SourceAffinity::After,
            )
            .unwrap()
            .0 as u64;
        let mut token = None;
        loop {
            match finish_workspace_view_restore(editor, &invalid, &mut token) {
                Err(_) => break,
                Ok(false) => {
                    while editor.busy() {
                        assert!(Instant::now() < deadline);
                        editor.pump();
                        std::thread::yield_now();
                    }
                }
                Ok(true) => panic!("Invalid UTF-8 endpoint was accepted"),
            }
        }
        let WorkspaceEditor::Paged(paged) = &*editor else {
            unreachable!()
        };
        assert!(matches!(
            paged.selection_restore_status(token.unwrap()),
            bareline_editor_surface::paged_view::SelectionRestoreStatus::Failed(_)
        ));
        assert_eq!(
            paged.global_selection(),
            (bareline_document::TextOffset(2), bareline_document::TextOffset(9))
        );
        assert_eq!(editor.viewport().selection, before);
        let mut invalid = workspace_view_state(editor);
        invalid.scroll_byte = Some(u64::MAX);
        assert!(restore_workspace_view(editor, &invalid).is_err());
        assert_eq!(editor.viewport().selection, before);
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
        let selection = workspace.editors[0].viewport().selection;
        restored.pending_view_scroll[0] = Some(guarded);
        restored.pump(&mut workspace);
        assert_eq!(workspace.editors[0].viewport().selection, selection);
        assert!(
            workspace.editors[0]
                .viewport()
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
        let mut workspace = Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.new_document().unwrap();
        workspace.editors[0].enqueue(Input::Insert("alpha".into()));
        let deadline = Instant::now() + Duration::from_secs(5);
        while workspace.editors[0].busy() {
            workspace.pump();
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        let mut views = ViewsRuntime::default();
        views.split(&mut workspace, 0, Orientation::Vertical);
        views.activate(&mut workspace, &mut App::default(), 1);
        assert_eq!(
            bareline_app::accessibility::source_identity(views.secondary.as_ref().unwrap()),
            bareline_app::accessibility::source_identity(&workspace.editors[0])
        );
        views.secondary.as_mut().unwrap().enqueue(Input::SetCaret(1, false));
        while views.secondary.as_ref().unwrap().busy() {
            views.secondary.as_mut().unwrap().pump();
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        let requested = super::super::language::completion_target(&views, &workspace, 0).unwrap();
        workspace.editors[0].enqueue(Input::SetCaret(2, false));
        while workspace.editors[0].busy() {
            workspace.pump();
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(
            super::super::language::completion_target(&views, &workspace, 0),
            Some(requested.clone()),
            "inactive primary cursor must not redirect a secondary request"
        );
        views.secondary.as_mut().unwrap().enqueue(Input::SetCaret(3, false));
        while views.secondary.as_ref().unwrap().busy() {
            views.secondary.as_mut().unwrap().pump();
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_ne!(
            super::super::language::completion_target(&views, &workspace, 0),
            Some(requested.clone()),
            "selection movement makes completion stale"
        );
        views.secondary.as_mut().unwrap().enqueue(Input::SetCaret(1, false));
        while views.secondary.as_ref().unwrap().busy() {
            views.secondary.as_mut().unwrap().pump();
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(
            super::super::language::completion_target(&views, &workspace, 0),
            Some(requested.clone())
        );
        views.activate(&mut workspace, &mut App::default(), 0);
        assert_ne!(
            super::super::language::completion_target(&views, &workspace, 0),
            Some(requested),
            "pane identity must prevent accepting into another clone"
        );
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
        views.activate(&mut workspace, &mut App::default(), 0);
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
        assert_eq!(views.secondary.as_ref().unwrap().persisted_folds(), vec![0..3]);
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
    fn discovered_fold_results_leave_the_document_expanded() {
        // Fold discovery on open requests level 0; its results used to
        // collapse every region of the document.
        let mut workspace = Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.new_document().unwrap();
        workspace.editors[0].enqueue(Input::Insert("a {\nb\n}\nc {\nd\n}\n".into()));
        let deadline = Instant::now() + Duration::from_secs(5);
        while workspace.editors[0].busy() {
            assert!(Instant::now() < deadline);
            workspace.pump();
            std::thread::yield_now();
        }
        let mut views = ViewsRuntime::default();
        views.split(&mut workspace, 0, Orientation::Vertical);
        let source = workspace.editors[0].snapshot().clone();
        let folds = || {
            vec![
                bareline_syntax::folding::Fold {
                    header: 0,
                    end: 2,
                    level: 1,
                },
                bareline_syntax::folding::Fold {
                    header: 3,
                    end: 5,
                    level: 1,
                },
            ]
        };
        views.record_fold_target();
        views.apply_fold_result(&mut workspace, &source, folds(), 0, false);
        assert!(views.secondary.as_ref().unwrap().persisted_folds().is_empty());
        assert!(workspace.editors[0].persisted_folds().is_empty());
        // A Fold All request (level 1) still collapses every region.
        views.record_fold_target();
        views.apply_fold_result(&mut workspace, &source, folds(), 1, false);
        assert_eq!(views.secondary.as_ref().unwrap().persisted_folds(), vec![0..3, 3..6]);
    }

    #[test]
    fn promotion_rebinds_linked_views_without_replacing_tabs_or_history() {
        let mut workspace = Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.new_document().unwrap();
        workspace.editors[0].enqueue(Input::Insert("alpha\nbeta\n".into()));
        let deadline = Instant::now() + Duration::from_secs(30);
        while workspace.editors[0].busy() {
            assert!(Instant::now() < deadline);
            workspace.pump();
            std::thread::yield_now();
        }
        let mut views = ViewsRuntime::default();
        views.split(&mut workspace, 0, Orientation::Vertical);
        workspace.editors[0].enqueue(Input::SetCaret(2, false));
        views.secondary.as_mut().unwrap().enqueue(Input::SetCaret(8, false));
        while views.busy(&workspace) {
            assert!(Instant::now() < deadline);
            workspace.pump();
            views.pump(&mut workspace);
            std::thread::yield_now();
        }
        assert_eq!(workspace.editors[0].viewport().selection.caret, 2);
        assert_eq!(views.secondary.as_ref().unwrap().viewport().selection.caret, 8);
        let ids = views.loaded_tabs;
        let identity = workspace.editors[0].snapshot().identity_token();
        assert!(!workspace.promote_resident_for_source_edit(0, identity).unwrap());
        loop {
            assert!(
                Instant::now() < deadline,
                "promotion convergence: message={:?}; primary paged={} busy={} error={:?}; secondary paged={} busy={} error={:?}; restore={:?}; scroll={:?}",
                workspace.message,
                workspace.editors[0].paged(),
                workspace.editors[0].busy(),
                workspace.editors[0].viewport().error,
                views.secondary.as_ref().is_some_and(WorkspaceEditor::paged),
                views.secondary.as_ref().is_some_and(WorkspaceEditor::busy),
                views.secondary.as_ref().and_then(|e| e.viewport().error.as_ref()),
                views.pending_restore.iter().map(Option::is_some).collect::<Vec<_>>(),
                views
                    .pending_view_scroll
                    .iter()
                    .map(Option::is_some)
                    .collect::<Vec<_>>()
            );
            workspace.pump();
            views.pump(&mut workspace);
            workspace
                .promote_resident_for_source_edit(0, identity)
                .unwrap_or_else(|error| panic!("promotion failed before view rebind: {error}"));
            if workspace.editors[0].paged()
                && views.secondary.as_ref().is_some_and(WorkspaceEditor::paged)
                && !views.busy(&workspace)
                && views.pending_restore.iter().all(Option::is_none)
                && views.pending_view_scroll.iter().all(Option::is_none)
            {
                break;
            }
            std::thread::yield_now();
        }
        assert_eq!(views.loaded_tabs, ids);
        let WorkspaceEditor::Paged(primary) = &workspace.editors[0] else {
            unreachable!()
        };
        let WorkspaceEditor::Paged(peer) = views.secondary.as_ref().unwrap() else {
            unreachable!()
        };
        assert!(primary.snapshot().same_document(peer.snapshot()));
        assert_eq!(
            primary.global_selection().1.0,
            2,
            "promotion selection error: {:?}, message: {:?}",
            primary.error,
            workspace.message
        );
        assert_eq!(peer.global_selection().1.0, 8);
        assert!(primary.can_undo());
        views.secondary.as_mut().unwrap().enqueue(Input::Insert("X".into()));
        loop {
            assert!(Instant::now() < deadline);
            workspace.pump();
            views.pump(&mut workspace);
            if !views.busy(&workspace) {
                break;
            }
            std::thread::yield_now();
        }
        let WorkspaceEditor::Paged(primary) = &workspace.editors[0] else {
            unreachable!()
        };
        let WorkspaceEditor::Paged(peer) = views.secondary.as_ref().unwrap() else {
            unreachable!()
        };
        assert_eq!(primary.snapshot().revision, peer.snapshot().revision);
        assert_eq!(primary.snapshot().len(), 12);
        workspace.editors[0].enqueue(Input::Undo);
        loop {
            assert!(Instant::now() < deadline);
            workspace.pump();
            views.pump(&mut workspace);
            if !views.busy(&workspace) {
                break;
            }
            std::thread::yield_now();
        }
        let WorkspaceEditor::Paged(primary) = &workspace.editors[0] else {
            unreachable!()
        };
        assert_eq!(primary.snapshot().len(), 11);
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
            assert!(Instant::now() < deadline, "pane edit acknowledgment timed out");
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
        views.secondary.as_mut().unwrap().set_logical_scroll(0, 0.0, 13.0);
        views.controller.as_mut().unwrap().sync_vertical = true;
        views.sync_scroll(&mut workspace, 0);
        assert_eq!(views.secondary.as_ref().unwrap().logical_scroll().2, 13.0);
        views.controller.as_mut().unwrap().sync_horizontal = true;
        views.sync_scroll(&mut workspace, 0);
        assert_eq!(views.secondary.as_ref().unwrap().logical_scroll().2, 84.0);
        views.secondary.as_mut().unwrap().viewport_mut().selection.anchor = 6;
        views.secondary.as_mut().unwrap().viewport_mut().scroll_y = 40.0;
        views.controller.as_mut().unwrap().ratio = 0.65;
        let mut manifest = bareline_file_io::session::SessionManifest::default();
        manifest.documents.push(bareline_file_io::session::SessionDocument {
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
        assert_eq!(restored.secondary.as_ref().unwrap().viewport().selection.anchor, 6);
        assert_eq!(restored.secondary.as_ref().unwrap().viewport().scroll_y, 40.0);
        assert_eq!(restored.controller.as_ref().unwrap().ratio, 0.65);
        views.collapse(&mut workspace, true);
        assert!(!views.open());
        assert_eq!(workspace.editors[0].snapshot().revision, primary.revision);
    }

    #[test]
    fn closing_split_preserves_shared_undo_redo_from_either_writer() {
        fn exercise(writer: u32, close_from: u32) {
            let mut workspace = Workspace::new(
                std::sync::Arc::new(|| {}),
                std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
            )
            .unwrap();
            workspace.new_document().unwrap();
            workspace.editors[0].enqueue(Input::Insert("base".into()));
            let deadline = Instant::now() + Duration::from_secs(5);
            while workspace.editors[0].busy() {
                assert!(Instant::now() < deadline, "initial edit timed out");
                workspace.pump();
                std::thread::yield_now();
            }
            let mut views = ViewsRuntime::default();
            let mut app = App::default();
            views.split(&mut workspace, 0, Orientation::Vertical);
            views.input(&mut workspace, 1 - writer, Input::SetCaret(1, false));
            views.activate(&mut workspace, &mut app, writer);
            views.input(&mut workspace, writer, Input::End(false));
            views.input(&mut workspace, writer, Input::Insert("!".into()));
            while views.busy(&workspace) {
                assert!(Instant::now() < deadline, "split edit timed out");
                workspace.pump();
                views.pump(&mut workspace);
                std::thread::yield_now();
            }
            views.activate(&mut workspace, &mut app, close_from);
            views.close_split(&mut workspace);
            assert!(!views.open());
            assert!(workspace.editors[0].can_undo());

            for (input, expected, caret) in [(Input::Undo, "base", 4), (Input::Redo, "base!", 5)] {
                workspace.editors[0].enqueue(input);
                while workspace.editors[0].busy() {
                    assert!(Instant::now() < deadline, "history edit timed out");
                    workspace.pump();
                    std::thread::yield_now();
                }
                let editor = &workspace.editors[0];
                let snapshot = editor.snapshot();
                assert_eq!(
                    snapshot
                        .read(
                            bareline_document::TextOffset(0)..bareline_document::TextOffset(snapshot.len()),
                            100
                        )
                        .unwrap(),
                    expected
                );
                assert_eq!(editor.viewport().selection.caret, caret);
            }
        }

        exercise(0, 1);
        exercise(1, 0);
    }

    #[test]
    fn tab_at_carets_preserves_line_prefix_and_shared_history() {
        for pane in [None, Some(0), Some(1)] {
            let mut workspace = Workspace::new(
                std::sync::Arc::new(|| {}),
                std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
            )
            .unwrap();
            workspace.new_document().unwrap();
            let mut views = ViewsRuntime::default();
            let mut app = App::default();
            let settle = |workspace: &mut Workspace, views: &mut ViewsRuntime| {
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    workspace.pump();
                    views.pump(workspace);
                    if !workspace.editors[0].busy() && !views.busy(workspace) {
                        break;
                    }
                    assert!(Instant::now() < deadline, "Tab edit timed out");
                    std::thread::yield_now();
                }
            };
            workspace.editors[0].enqueue(Input::Insert("hello world".into()));
            settle(&mut workspace, &mut views);
            if let Some(pane) = pane {
                views.split(&mut workspace, 0, Orientation::Vertical);
                views.activate(&mut workspace, &mut app, pane);
                views.input(&mut workspace, pane, Input::SetCaret(5, false));
            } else {
                workspace.editors[0].enqueue(Input::SetCaret(5, false));
            }
            settle(&mut workspace, &mut views);
            assert!(views.insert_tab_at_carets(&mut workspace, 0));
            settle(&mut workspace, &mut views);
            let text = |editor: &WorkspaceEditor| {
                let snapshot = editor.snapshot();
                snapshot
                    .read(
                        bareline_document::TextOffset(0)..bareline_document::TextOffset(snapshot.len()),
                        100,
                    )
                    .unwrap()
            };
            assert_eq!(text(&workspace.editors[0]), "hello\t world");
            if let Some(peer) = &views.secondary {
                assert_eq!(text(peer), "hello\t world");
            }
            if let Some(pane) = pane {
                views.input(&mut workspace, pane, Input::Undo);
            } else {
                workspace.editors[0].enqueue(Input::Undo);
            }
            settle(&mut workspace, &mut views);
            assert_eq!(text(&workspace.editors[0]), "hello world");
            let editor = views.active_workspace_editor_mut(&mut workspace, 0).unwrap();
            editor.viewport_mut().selection = bareline_editor_surface::Selection { anchor: 0, caret: 5 };
            assert!(
                !views.insert_tab_at_carets(&mut workspace, 0),
                "selected text keeps Indent command semantics"
            );
            assert_eq!(text(&workspace.editors[0]), "hello world");
        }
    }

    #[test]
    fn paged_tab_at_carets_edits_the_active_clone_and_preserves_selection_indent() {
        let path = std::env::temp_dir().join(format!("bareline-paged-tab-{}.txt", std::process::id()));
        std::fs::write(&path, "hello world\nsecond line").unwrap();
        let mut workspace = Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.resident_max_bytes = 1;
        workspace.open(path.clone());
        let mut views = ViewsRuntime::default();
        let mut app = App::default();
        let settle = |workspace: &mut Workspace, views: &mut ViewsRuntime| {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                workspace.pump();
                views.pump(workspace);
                let ready = |editor: &WorkspaceEditor| matches!(editor, WorkspaceEditor::Paged(paged) if paged.viewport_ready() && !paged.busy());
                if workspace.editors.first().is_some_and(ready)
                    && views.secondary.as_ref().is_none_or(ready)
                    && !views.busy(workspace)
                    && views.pending_restore.iter().all(Option::is_none)
                    && views.pending_view_scroll.iter().all(Option::is_none)
                {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "paged Tab did not settle: {:?}",
                    workspace.message
                );
                std::thread::yield_now();
            }
        };
        settle(&mut workspace, &mut views);
        views.split(&mut workspace, 0, Orientation::Vertical);
        settle(&mut workspace, &mut views);
        for pane in [0, 1] {
            views.activate(&mut workspace, &mut app, pane);
            views.input(&mut workspace, pane, Input::SetCaret(5, false));
            settle(&mut workspace, &mut views);
            assert!(views.insert_tab_at_carets(&mut workspace, 0));
            settle(&mut workspace, &mut views);
            for editor in [&workspace.editors[0], views.secondary.as_ref().unwrap()] {
                let snapshot = editor.snapshot();
                assert_eq!(
                    snapshot
                        .read(bareline_document::TextOffset(0)..bareline_document::TextOffset(12), 100)
                        .unwrap(),
                    "hello\t world"
                );
            }
            views.input(&mut workspace, pane, Input::Undo);
            settle(&mut workspace, &mut views);
            views.input(&mut workspace, pane, Input::SetCaret(0, false));
            views.input(&mut workspace, pane, Input::SetCaret(5, true));
            settle(&mut workspace, &mut views);
            assert!(!views.insert_tab_at_carets(&mut workspace, 0));
        }
        views.close_split(&mut workspace);
        drop(views);
        drop(workspace);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn pane_source_generation_tracks_full_document_replacement_and_revision() {
        let mut workspace = Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.new_document().unwrap();
        workspace.new_document().unwrap();
        let mut views = ViewsRuntime::default();
        views.sync_documents(&workspace);
        views.install_views(&mut workspace);
        let tab = views.pane_token(0).unwrap();
        let initial = views.accessibility_source_identity(0, &workspace.editors[0]).unwrap();
        workspace.editors[0].viewport_mut().scroll_y = 40.0;
        assert_eq!(
            views.accessibility_source_identity(0, &workspace.editors[0]),
            Some(initial),
            "selection and viewport state are not document-source revisions"
        );

        workspace.editors[0].enqueue(Input::Insert("changed".into()));
        while workspace.editors[0].busy() {
            workspace.pump();
        }
        let edited = views.accessibility_source_identity(0, &workspace.editors[0]).unwrap();
        assert_ne!(edited, initial);
        assert_eq!(edited.0, super::super::accessibility::editor_provider_id(tab));

        let replacement = views.accessibility_source_identity(0, &workspace.editors[1]).unwrap();
        assert_ne!(
            replacement, edited,
            "a replacement document with reset revision retires the reader"
        );
        assert_eq!(
            replacement.0, edited.0,
            "the stable view node survives document replacement"
        );
    }

    #[test]
    fn closed_view_retires_its_source_generation_without_recycling_the_survivor() {
        let mut workspace = Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.new_document().unwrap();
        let mut views = ViewsRuntime::default();
        views.split(&mut workspace, 0, Orientation::Vertical);
        let closed = views.pane_token(0).unwrap();
        let survivor = views.pane_token(1).unwrap();
        for pane in 0..2 {
            let editor = views.pane_workspace_editor(&workspace, 0, pane).unwrap();
            views.accessibility_source_identity(pane, editor).unwrap();
        }
        assert_eq!(views.accessibility_sources.borrow().len(), 2);

        let mut app = App::default();
        views.close_tab(&mut workspace, &mut app, closed);

        let sources = views.accessibility_sources.borrow();
        assert!(!sources.contains_key(&closed));
        assert!(sources.contains_key(&survivor));
    }

    #[test]
    fn closing_secondary_tab_returns_its_document_history() {
        let mut workspace = Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.new_document().unwrap();
        let mut views = ViewsRuntime::default();
        let mut app = App::default();
        views.split(&mut workspace, 0, Orientation::Vertical);
        views.activate(&mut workspace, &mut app, 1);
        views.input(&mut workspace, 1, Input::Insert("Z".into()));
        let deadline = Instant::now() + Duration::from_secs(5);
        while views.busy(&workspace) {
            assert!(Instant::now() < deadline, "secondary edit timed out");
            workspace.pump();
            views.pump(&mut workspace);
            std::thread::yield_now();
        }
        let closed = views.pane_token(1).unwrap();
        views.close_tab(&mut workspace, &mut app, closed);
        assert!(workspace.editors[0].can_undo());

        for (input, expected) in [(Input::Undo, ""), (Input::Redo, "Z")] {
            workspace.editors[0].enqueue(input);
            while workspace.editors[0].busy() {
                assert!(Instant::now() < deadline, "returned history timed out");
                workspace.pump();
                std::thread::yield_now();
            }
            let snapshot = workspace.editors[0].snapshot();
            assert_eq!(
                snapshot
                    .read(
                        bareline_document::TextOffset(0)..bareline_document::TextOffset(snapshot.len()),
                        10
                    )
                    .unwrap(),
                expected
            );
        }
    }

    #[test]
    fn close_compare_waits_for_secondary_history_to_settle() {
        let mut workspace = Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.new_document().unwrap();
        workspace.new_document().unwrap();
        let mut views = ViewsRuntime::default();
        assert!(views.compare_pair(&mut workspace, 0, 1));
        views.input(&mut workspace, 1, Input::Insert("Z".into()));
        assert!(views.busy(&workspace));

        views.close_compare(&mut workspace);
        assert!(views.compare);
        assert!(views.open());
        assert!(views.secondary.is_some());
        assert_eq!(
            workspace.message.as_deref(),
            Some("Wait for pending edits before closing the comparison.")
        );

        let deadline = Instant::now() + Duration::from_secs(5);
        while views.busy(&workspace) {
            assert!(Instant::now() < deadline, "compare edit timed out");
            workspace.pump();
            views.pump(&mut workspace);
            std::thread::yield_now();
        }
        views.close_compare(&mut workspace);
        assert!(!views.compare);
        assert!(!views.open());
        assert!(workspace.editors[1].can_undo());

        for (input, expected) in [(Input::Undo, ""), (Input::Redo, "Z")] {
            workspace.editors[1].enqueue(input);
            while workspace.editors[1].busy() {
                assert!(Instant::now() < deadline, "compare history timed out");
                workspace.pump();
                std::thread::yield_now();
            }
            let snapshot = workspace.editors[1].snapshot();
            assert_eq!(
                snapshot
                    .read(
                        bareline_document::TextOffset(0)..bareline_document::TextOffset(snapshot.len()),
                        10
                    )
                    .unwrap(),
                expected
            );
        }
    }

    #[test]
    fn split_pane_activation_retires_the_previous_documents_find_results() {
        let (notifier, notified) = std::sync::mpsc::channel();
        let notify: std::sync::Arc<dyn Fn() + Send + Sync> = std::sync::Arc::new(move || {
            let _ = notifier.send(());
        });
        let mut workspace = Workspace::new(
            notify.clone(),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.new_document().unwrap();
        workspace.new_document().unwrap();
        workspace.editors[0].enqueue(Input::Insert("needle needle".into()));
        workspace.editors[1].enqueue(Input::Insert("other".into()));
        while workspace.editors.iter().any(WorkspaceEditor::busy) {
            notified.recv_timeout(Duration::from_secs(5)).unwrap();
            workspace.pump();
        }
        let mut views = ViewsRuntime::default();
        views.sync_documents(&workspace);
        let second = views.controller.as_ref().unwrap().tabs()[1].id;
        views.controller.as_mut().unwrap().move_to_other(second).unwrap();
        views.install_views(&mut workspace);
        let mut app = App::default();
        views.activate(&mut workspace, &mut app, 0);
        workspace.find.show_replace();
        workspace.find.field.insert("needle");
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        let mut operations = Vec::new();
        views
            .draw(
                &mut workspace,
                &mut app,
                &mut renderer,
                1100.0,
                700.0,
                &mut operations,
                notify,
            )
            .unwrap();
        while workspace.find.searching() {
            notified.recv_timeout(Duration::from_secs(5)).unwrap();
            workspace.pump();
        }
        assert_eq!(workspace.find.completed_results().unwrap().count(), 2);

        views.activate(&mut workspace, &mut app, 1);

        assert!(workspace.find.completed_results().is_none());
        assert_eq!(workspace.find.status, "Searching…");
        assert!(
            workspace
                .find
                .semantics(1100.0)
                .into_iter()
                .find(|node| node.name == "Replace All")
                .unwrap()
                .disabled
        );
    }

    fn tab_at(views: &ViewsRuntime, workspace: &Workspace, index: usize) -> u64 {
        views
            .controller
            .as_ref()
            .unwrap()
            .tabs()
            .iter()
            .find(|tab| views.tab_index(workspace, tab.id) == Some(index))
            .unwrap()
            .id
    }

    /// PED-23: a large-file open that finishes in its own (here failed) tab
    /// keeps that tab's identity, position, pin and colour, and the tab the
    /// user moved to stays active.
    #[test]
    fn finished_paged_open_keeps_its_tab_position_pin_colour_and_focus() {
        let root = std::env::temp_dir().join(format!(
            "bareline-tab-stability-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("large.txt");
        let mut workspace = Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.new_document().unwrap();
        workspace.open(path.clone());
        let deadline = Instant::now() + Duration::from_secs(30);
        while workspace.io_busy() {
            assert!(Instant::now() < deadline, "{:?}", workspace.message);
            workspace.pump();
            std::thread::yield_now();
        }
        assert!(workspace.failed_open(1).is_some(), "{:?}", workspace.message);
        // The shell showed the failed tab as the open asked (APP-07); the user
        // then moves to another tab.
        assert_eq!(workspace.take_activation(None), Some(1));
        workspace.new_document().unwrap();
        let mut views = ViewsRuntime::default();
        let mut app = App::default();
        views.sync_documents(&workspace);
        let loading = tab_at(&views, &workspace, 1);
        let other = tab_at(&views, &workspace, 2);
        {
            let controller = views.controller.as_mut().unwrap();
            controller.pin(loading, true).unwrap();
            controller.color(loading, Some(0x36c9c6)).unwrap();
        }
        views.select_tab(&mut workspace, &mut app, other);
        assert_eq!(app.active, 2);
        app.tabs = workspace.titles();
        let order: Vec<u64> = views
            .controller
            .as_ref()
            .unwrap()
            .tabs()
            .iter()
            .map(|tab| tab.id)
            .collect();
        std::fs::write(&path, "line\n".repeat(2_000)).unwrap();
        workspace.open_failed_as_large_file(1).unwrap();
        // The shell's loop: pump, follow the active document, sync the views.
        loop {
            assert!(Instant::now() < deadline, "{:?}", workspace.message);
            let before = workspace.tab_documents();
            if workspace.pump() {
                app.active = workspace.active_after_pump(&before, app.active);
                app.tabs = workspace.titles();
            }
            views.sync(&mut workspace, &mut app);
            if !workspace.io_busy() && !workspace.editors.iter().any(WorkspaceEditor::busy) {
                break;
            }
            std::thread::yield_now();
        }
        assert!(
            matches!(&workspace.editors[1], WorkspaceEditor::Paged(_)),
            "{:?}",
            workspace.message
        );
        assert_eq!(app.tabs, ["Untitled 1", "large.txt", "Untitled 2"]);
        let controller = views.controller.as_ref().unwrap();
        assert_eq!(controller.tabs().iter().map(|tab| tab.id).collect::<Vec<_>>(), order);
        assert!(controller.tab(loading).unwrap().pinned);
        assert_eq!(controller.tab_colors.get(&loading), Some(&0x36c9c6));
        assert_eq!(views.tab_index(&workspace, loading), Some(1));
        assert_eq!(app.active, 2);
        assert_eq!(controller.active_tab(0), Some(other));
        drop(views);
        drop(workspace);
        let _ = std::fs::remove_dir_all(root);
    }

    /// PED-23/WSP-11: Reload gives a tab a fresh document in place. The tab
    /// keeps its identity, position, pin and colour instead of closing and
    /// reappearing at the end, and stays the active tab.
    #[test]
    fn reloaded_tab_keeps_its_position_pin_colour_and_focus() {
        let root = std::env::temp_dir().join(format!(
            "bareline-reload-tab-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("reload.txt");
        std::fs::write(&path, "alpha\n").unwrap();
        let mut workspace = Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.new_document().unwrap();
        workspace.open(path.clone());
        let deadline = Instant::now() + Duration::from_secs(30);
        while workspace.io_busy() || workspace.editors.iter().any(WorkspaceEditor::busy) {
            assert!(Instant::now() < deadline, "{:?}", workspace.message);
            workspace.pump();
            std::thread::yield_now();
        }
        assert_eq!(workspace.path(1), Some(path.as_path()), "{:?}", workspace.message);
        workspace.new_document().unwrap();
        let mut views = ViewsRuntime::default();
        let mut app = App::default();
        views.sync_documents(&workspace);
        let reloaded = tab_at(&views, &workspace, 1);
        {
            let controller = views.controller.as_mut().unwrap();
            controller.pin(reloaded, true).unwrap();
            controller.color(reloaded, Some(0x36c9c6)).unwrap();
        }
        views.select_tab(&mut workspace, &mut app, reloaded);
        assert_eq!(app.active, 1);
        app.tabs = workspace.titles();
        let order: Vec<u64> = views
            .controller
            .as_ref()
            .unwrap()
            .tabs()
            .iter()
            .map(|tab| tab.id)
            .collect();
        let old = workspace.editors[1].document_identity();
        std::fs::write(&path, "beta\n").unwrap();
        workspace.reload(1, false).unwrap();
        // The shell's loop: pump, follow the active document, sync the views.
        loop {
            assert!(Instant::now() < deadline, "{:?}", workspace.message);
            let before = workspace.tab_documents();
            if workspace.pump() {
                app.active = workspace.active_after_pump(&before, app.active);
                app.tabs = workspace.titles();
            }
            views.sync(&mut workspace, &mut app);
            if !workspace.io_busy() && !workspace.editors.iter().any(WorkspaceEditor::busy) {
                break;
            }
            std::thread::yield_now();
        }
        assert_ne!(workspace.editors[1].document_identity(), old, "{:?}", workspace.message);
        assert_eq!(workspace.editors[1].snapshot().len(), "beta\n".len());
        let controller = views.controller.as_ref().unwrap();
        assert_eq!(controller.tabs().iter().map(|tab| tab.id).collect::<Vec<_>>(), order);
        assert!(controller.tab(reloaded).unwrap().pinned);
        assert_eq!(controller.tab_colors.get(&reloaded), Some(&0x36c9c6));
        assert_eq!(views.tab_index(&workspace, reloaded), Some(1));
        assert_eq!(app.active, 1);
        assert_eq!(controller.active_tab(0), Some(reloaded));
        drop(views);
        drop(workspace);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn restore_closed_tab_brings_back_pin_position_view_read_only_and_focus() {
        let root = std::env::temp_dir().join(format!(
            "bareline-restore-closed-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let other = root.join("other.txt");
        let saved = root.join("saved.txt");
        std::fs::write(&other, "other\n").unwrap();
        std::fs::write(&saved, "alpha\nbeta\ngamma\n").unwrap();
        let mut shell = crate::windows_app::accessibility::tests::headless_shell();
        let mut workspace = Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.open(other.clone());
        workspace.open(saved.clone());
        shell.workspace = Some(workspace);
        let settle = |shell: &mut Shell| {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            loop {
                let before = shell.workspace.as_ref().unwrap().tab_documents();
                if shell.workspace.as_mut().unwrap().pump() {
                    shell.follow_workspace_activation(&before);
                }
                let workspace = shell.workspace.as_ref().unwrap();
                if !workspace.io_busy() && !workspace.editors.iter().any(WorkspaceEditor::busy) {
                    break;
                }
                assert!(std::time::Instant::now() < deadline, "{:?}", workspace.message);
                std::thread::yield_now();
            }
        };
        settle(&mut shell);
        let workspace = shell.workspace.as_mut().unwrap();
        let index = (0..workspace.editors.len())
            .find(|index| workspace.path(*index) == Some(saved.as_path()))
            .unwrap();
        shell.views.sync_documents(workspace);
        let id = shell
            .views
            .controller
            .as_ref()
            .unwrap()
            .tabs()
            .iter()
            .map(|tab| tab.id)
            .find(|id| shell.views.tab_index(workspace, *id) == Some(index))
            .unwrap();
        let controller = shell.views.controller.as_mut().unwrap();
        let mut view = controller.tab(id).unwrap().view.clone();
        // The caret sits inside "beta".
        view.anchor = 8;
        view.caret = 8;
        controller.set_view_state(id, view).unwrap();
        controller.pin(id, true).unwrap();
        let position = controller.tabs().iter().position(|tab| tab.id == id).unwrap();
        workspace.editors[index].set_read_only(true);
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        workspace.close(index, false, &mut renderer).unwrap();
        workspace.set_last_closed_read_only(true);
        shell.views.sync_documents(workspace);
        assert!(shell.views.controller.as_ref().unwrap().tab(id).is_none());
        shell.app.active = 0;

        // Once a worker finds its file (APP-19), a saved file reopens from disk
        // as a new document (WSP-05).
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while workspace.closed_checks_pending() {
            workspace.pump();
            assert!(
                std::time::Instant::now() < deadline,
                "the closed file was never checked"
            );
            std::thread::yield_now();
        }
        assert_eq!(workspace.restore_last_closed(), None);
        settle(&mut shell);
        let workspace = shell.workspace.as_mut().unwrap();
        let restored = (0..workspace.editors.len())
            .find(|index| workspace.path(*index) == Some(saved.as_path()))
            .unwrap();
        assert_eq!(shell.app.active, restored, "the restored tab is activated");
        assert!(workspace.editors[restored].read_only());
        shell.views.sync(workspace, &mut shell.app);
        let controller = shell.views.controller.as_ref().unwrap();
        let tab = controller.tab(id).expect("the closed tab comes back");
        assert!(tab.pinned);
        assert_eq!(controller.tabs().iter().position(|tab| tab.id == id), Some(position));
        assert_eq!(shell.views.tab_index(workspace, id), Some(restored));
        assert_eq!(workspace.editors[restored].viewport().selection.caret, 8);
        drop(shell);
        let _ = std::fs::remove_dir_all(root);
    }

    fn overlaps(a: Rect, b: Rect) -> bool {
        a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
    }

    /// UI-08 acceptance: 30 tabs at 1200 px shrink to share the strip (11
    /// visible instead of 7 fixed 150 px tabs), stay clear of the overflow
    /// buttons, and every tab is reachable through the list of all tabs.
    #[test]
    fn thirty_tabs_at_1200_px_shrink_and_list_every_tab() {
        let mut workspace = Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        for _ in 0..30 {
            workspace.new_document().unwrap();
        }
        let mut views = ViewsRuntime::default();
        views.sync_documents(&workspace);
        let strip = rect(0.0, 0.0, 1200.0, TAB_HEIGHT);
        let mut operations = Vec::new();
        views.draw_tab_strip(&workspace, 0, strip, false, &mut operations);
        assert!(
            views.tab_hits.len() >= 11,
            "{} of 30 tabs visible",
            views.tab_hits.len()
        );
        assert_eq!(
            views.tab_lists.len(),
            1,
            "an overflowing strip offers the list of all tabs"
        );
        let (pane, list) = views.tab_lists[0];
        assert!(list.x + list.width <= strip.width + 0.01);
        for hit in &views.tab_hits {
            assert!(hit.bounds.width >= bareline_ui::controls::TabStrip::MIN_TAB_WIDTH);
            assert!(hit.bounds.x + hit.bounds.width <= strip.width - TAB_NAV_RESERVE + 0.01);
            assert!(!overlaps(hit.bounds, list));
        }
        for (_, _, nav) in &views.tab_nav {
            assert!(!overlaps(*nav, list));
        }

        views.open_tab_list(pane);
        let order: Vec<u64> = views
            .controller
            .as_ref()
            .unwrap()
            .pane_tabs(0)
            .map(|tab| tab.id)
            .collect();
        let popup = views.mru_popup.as_ref().unwrap();
        assert!(popup.list);
        assert_eq!(popup.ids, order, "the list holds every tab in strip order");
        views.draw_mru(&workspace, 1200.0, 800.0, &mut operations);
        let bounds = views.mru_popup.as_ref().unwrap().bounds;
        assert!(bounds.y >= TAB_HEIGHT && bounds.x + bounds.width <= 1200.0);

        // Choosing a tab that was off the strip selects it and scrolls it in.
        let last = *order.last().unwrap();
        assert!(views.tab_hits.iter().all(|hit| hit.id != last));
        views.mru_popup = None;
        let mut app = App::default();
        views.select_tab(&mut workspace, &mut app, last);
        assert_eq!(app.active, 29);
        views.tab_hits.clear();
        views.tab_nav.clear();
        views.tab_lists.clear();
        views.draw_tab_strip(&workspace, 0, strip, false, &mut operations);
        assert!(views.tab_hits.iter().any(|hit| hit.id == last));
    }

    /// UI-08: strip widths that do not divide evenly by the tab count still
    /// show every tab that fits, without the overflow controls. 7 tabs at
    /// 1054 px used to lose one to f32 rounding of `982 / (982 / 7)`.
    #[test]
    fn tabs_that_fit_uneven_widths_all_stay_visible() {
        for (width, count) in [(1054.0, 7), (1103.0, 7), (1200.0, 9)] {
            let mut workspace = Workspace::new(
                std::sync::Arc::new(|| {}),
                std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
            )
            .unwrap();
            for _ in 0..count {
                workspace.new_document().unwrap();
            }
            let mut views = ViewsRuntime::default();
            views.sync_documents(&workspace);
            let strip = rect(0.0, 0.0, width, TAB_HEIGHT);
            let mut operations = Vec::new();
            views.draw_tab_strip(&workspace, 0, strip, false, &mut operations);
            assert_eq!(views.tab_hits.len(), count, "{count} tabs at {width} px");
            assert!(views.tab_lists.is_empty(), "no overflow list at {width} px");
            assert!(views.tab_nav.is_empty(), "no scroll arrows at {width} px");
            for hit in &views.tab_hits {
                assert!(hit.bounds.x + hit.bounds.width <= width - TAB_NAV_RESERVE + 0.01);
            }
        }
    }

    /// UI-08: the Settings and Extensions tabs join their strip's UIA set after
    /// its documents, with their position and the size of the whole set.
    #[test]
    fn page_tabs_are_in_the_strip_set_for_screen_readers() {
        use bareline_platform::accessibility::AccessibilityRole;
        let mut shell = super::super::accessibility::tests::headless_shell();
        let mut workspace = Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.new_document().unwrap();
        workspace.new_document().unwrap();
        shell.views.sync_documents(&workspace);
        shell.views.install_views(&mut workspace);
        shell.workspace = Some(workspace);
        // Settings is shown; Extensions keeps a parked tab.
        shell.views.set_open_pages(true, false);
        shell.views.park_page(PageTab::Extensions);
        let mut operations = Vec::new();
        shell.views.draw_tab_strip(
            shell.workspace.as_ref().unwrap(),
            0,
            rect(0.0, 0.0, 1000.0, TAB_HEIGHT),
            false,
            &mut operations,
        );

        let nodes = shell.views_accessibility_nodes();
        let tabs: Vec<_> = nodes
            .iter()
            .filter(|node| node.role == AccessibilityRole::Tab && node.parent == ACCESS_STRIP_BASE)
            .collect();
        assert_eq!(tabs.len(), 4, "two documents and two page tabs");
        for (index, tab) in tabs.iter().enumerate() {
            assert_eq!(tab.position_in_set, Some(index + 1));
            assert_eq!(tab.size_of_set, Some(4));
        }
        assert_eq!(tabs[2].id, PageTab::Settings.access_id());
        assert_eq!(tabs[2].name, "Settings");
        assert!(tabs[2].selected && tabs[2].invokable && tabs[2].bounds[2] > 0.0);
        assert_eq!(tabs[3].id, PageTab::Extensions.access_id());
        assert_eq!(tabs[3].name, "Extensions");
        assert!(!tabs[3].selected);
        // While a page is shown no document tab reads as selected, as drawn.
        assert!(tabs[..2].iter().all(|tab| !tab.selected));
        assert!(nodes.iter().any(|node| {
            node.id == PageTab::Extensions.access_id() + 1
                && node.parent == PageTab::Extensions.access_id()
                && node.role == AccessibilityRole::Button
                && node.name == "Close Extensions"
        }));
        // Every id in the strip is distinct from the document tab ids.
        let mut ids: Vec<_> = nodes.iter().map(|node| node.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), nodes.len());

        // The page tab's × closes the page and drops its tab.
        shell
            .views
            .close_page(PageTab::Extensions, &mut shell.settings, &mut shell.extensions);
        assert_eq!(shell.views.page_tabs(), [PageTab::Settings]);
    }

    /// UI-05/UI-09: Settings and Extensions are tabs in the strip, and
    /// activating a document from the Window menu leaves either page while
    /// its tab stays available.
    #[test]
    fn activating_a_document_leaves_the_page_and_keeps_its_tab() {
        let mut shell = super::super::accessibility::tests::headless_shell();
        let mut workspace = Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.new_document().unwrap();
        workspace.new_document().unwrap();
        shell.views.sync_documents(&workspace);
        shell.views.install_views(&mut workspace);
        shell.workspace = Some(workspace);

        shell.settings.controller.show();
        shell
            .views
            .set_open_pages(shell.settings.controller.open, shell.extensions.open);
        assert_eq!(shell.views.page_tabs(), [PageTab::Settings]);
        shell.select_window_document(1);
        assert!(!shell.settings.controller.open, "the Window menu leaves Settings");
        assert_eq!(shell.app.active, 1);
        shell
            .views
            .set_open_pages(shell.settings.controller.open, shell.extensions.open);
        assert_eq!(shell.views.page_tabs(), [PageTab::Settings], "Settings keeps its tab");

        shell.extensions.open = true;
        shell
            .views
            .set_open_pages(shell.settings.controller.open, shell.extensions.open);
        shell.select_window_document(0);
        assert!(!shell.extensions.open, "the Window menu leaves Extensions");
        assert_eq!(shell.app.active, 0);
        shell.views.set_open_pages(false, false);
        assert_eq!(shell.views.page_tabs(), [PageTab::Settings, PageTab::Extensions]);

        let mut operations = Vec::new();
        shell.views.draw_tab_strip(
            shell.workspace.as_ref().unwrap(),
            0,
            rect(0.0, 0.0, 1000.0, TAB_HEIGHT),
            false,
            &mut operations,
        );
        let (pages, documents): (Vec<TabHit>, Vec<TabHit>) = shell
            .views
            .tab_hits
            .iter()
            .partition(|hit| PageTab::from_tab_id(hit.id).is_some());
        assert_eq!(pages.len(), 2);
        assert_eq!(documents.len(), 2);
        for page in &pages {
            assert!(page.bounds.x + page.bounds.width <= 1000.0 - TAB_NAV_RESERVE + 0.01);
            for document in &documents {
                assert!(!overlaps(page.bounds, document.bounds));
            }
        }
        // Page tabs never become accessibility tab ids or drag targets.
        assert!(pages.iter().all(|page| access_tab_id(page.id).is_none()));
    }

    fn settle(workspace: &mut Workspace) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while workspace.io_busy() || workspace.editors.iter().any(WorkspaceEditor::busy) {
            workspace.pump();
            assert!(Instant::now() < deadline, "{:?}", workspace.message);
            std::thread::yield_now();
        }
    }

    /// FIO-01 follow-up: a failed open shown in the secondary pane paints its
    /// error panel there and presses reach its Retry action, not a text surface.
    #[test]
    fn failed_open_in_a_split_pane_paints_its_panel_and_routes_retry() {
        let root = std::env::temp_dir().join(format!(
            "bareline-split-failed-open-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let mut workspace = Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.new_document().unwrap();
        workspace.open(root.join("missing.txt"));
        settle(&mut workspace);
        assert!(workspace.failed_open(1).is_some(), "{:?}", workspace.message);
        let mut views = ViewsRuntime::default();
        views.sync_documents(&workspace);
        let failed = views.controller.as_ref().unwrap().tabs()[1].id;
        views.controller.as_mut().unwrap().move_to_other(failed).unwrap();
        views.install_views(&mut workspace);
        let mut app = App::default();
        views.activate(&mut workspace, &mut app, 1);
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        let mut operations = Vec::new();
        views
            .draw(
                &mut workspace,
                &mut app,
                &mut renderer,
                1100.0,
                700.0,
                &mut operations,
                std::sync::Arc::new(|| {}),
            )
            .unwrap();
        let pane = views.bounds[1].unwrap();
        assert!(operations.iter().any(|op| matches!(
            op,
            DrawOp::Text { origin, text, .. } if text.starts_with("Could not open") && origin.x >= pane.x
        )));
        // Its status reports that nothing is loaded, not an indexing state.
        assert_eq!(
            views.secondary.as_ref().unwrap().viewport().size_status_label(),
            "Not loaded"
        );
        let retry = Point {
            x: 26.0,
            y: TAB_HEIGHT + 126.0,
        };
        assert!(
            !views.failed_open_pointer(&mut workspace, 0, retry),
            "the untitled pane has no failed open"
        );
        assert!(views.failed_open_pointer(&mut workspace, 1, retry));
        drop(views);
        drop(workspace);
        let _ = std::fs::remove_dir_all(root);
    }

    /// FIO-01 follow-up: a failed open's placeholder is not loading, so the
    /// footer shows no indexing track for it.
    #[test]
    fn failed_open_placeholder_has_no_indexing_track() {
        let root = std::env::temp_dir().join(format!(
            "bareline-failed-open-track-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let mut workspace = Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.open(root.join("missing.txt"));
        settle(&mut workspace);
        assert!(workspace.failed_open(0).is_some(), "{:?}", workspace.message);
        let mut shell = super::super::accessibility::tests::headless_shell();
        shell.workspace = Some(workspace);
        shell.app.active = 0;
        let mut operations = Vec::new();
        shell.draw_footer(
            winit::dpi::PhysicalSize::new(1000, 700),
            1.0,
            &vec!["Plain text".to_string(); 6],
            &mut operations,
        );
        let track_y = 700.0 - 24.0;
        assert!(
            !operations.iter().any(|op| matches!(
                op,
                DrawOp::Fill(bounds, _) if bounds.y == track_y && bounds.height == 2.0
            )),
            "a failed open shows no loading track"
        );
        drop(shell);
        let _ = std::fs::remove_dir_all(root);
    }

    /// UI-07: a document-scoped notice stays visible when a long size label
    /// leaves no room for it in the status bar; only the generic hint drops.
    #[test]
    fn scoped_notice_survives_a_crowded_status_bar() {
        let mut workspace = Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.new_document().unwrap();
        let document = workspace.editors[0].document_identity();
        let mut shell = super::super::accessibility::tests::headless_shell();
        shell.toasts.enqueue(
            super::super::toast::Notification::new(
                "scoped-status-test",
                1,
                bareline_ui::theme::ToastLevel::Info,
                super::super::toast::NotificationKind::Progress,
                "Recovery snapshot preparing",
                None,
                Some(document),
                super::super::toast::NotificationLifetime::Scoped,
            ),
            std::time::Instant::now(),
        );
        shell.workspace = Some(workspace);
        shell.app.active = 0;
        let mut labels = vec!["Plain text".to_string(); 6];
        labels[1] = "1,234,567,890 lines scanned so far (partial)".into();
        for (width, in_bar) in [(1000u32, true), (480, false)] {
            let mut operations = Vec::new();
            shell.draw_footer(winit::dpi::PhysicalSize::new(width, 700), 1.0, &labels, &mut operations);
            let bar_y = 700.0 - 24.0;
            let notice = operations.iter().find_map(|op| match op {
                DrawOp::Text { origin, text, .. } if text.starts_with("Recovery") => Some(*origin),
                _ => None,
            });
            let notice = notice.unwrap_or_else(|| panic!("scoped notice drawn at {width} px"));
            assert_eq!(notice.y >= bar_y, in_bar, "notice placement at {width} px");
            if !in_bar {
                // Moved to an opaque pill above the bar, never bare over text.
                assert!(operations.iter().any(|op| matches!(
                    op,
                    DrawOp::Fill(bounds, _) if bounds.y == bar_y - 24.0 && bounds.contains(notice)
                )));
            }
        }
    }
}

/// Fixed pool of Window-menu slots, each bound to one open document. Surplus
/// slots are hidden; documents past the pool are reached via the document list.
pub(super) const WINDOW_CAP: usize = 20;
pub(super) const WINDOW_IDS: [&str; WINDOW_CAP] = [
    "window.select.0",
    "window.select.1",
    "window.select.2",
    "window.select.3",
    "window.select.4",
    "window.select.5",
    "window.select.6",
    "window.select.7",
    "window.select.8",
    "window.select.9",
    "window.select.10",
    "window.select.11",
    "window.select.12",
    "window.select.13",
    "window.select.14",
    "window.select.15",
    "window.select.16",
    "window.select.17",
    "window.select.18",
    "window.select.19",
];
pub(super) fn register(registry: &mut CommandRegistry) {
    for id in WINDOW_IDS {
        let id = CommandId(id);
        let registered = registry.register(CommandSpec {
            id,
            title: "Open Document",
            category: "Window",
            shortcut: "",
            action: Action::Contributed(id),
        });
        debug_assert!(registered.is_ok(), "duplicate command ID {id:?}");
    }
    for (id, title, shortcut) in [
        ("view.split_vertical", "Split Vertically", ""),
        ("view.split_horizontal", "Split Horizontally", ""),
        ("view.clone_other", "Clone to Other View", ""),
        ("view.move_other", "Move to Other View", ""),
        ("view.close_split", "Close Split View", ""),
        ("view.focus_other", "Focus Other View", "F6"),
        ("view.sync_vertical", "Synchronize Vertical Scrolling", ""),
        ("view.sync_horizontal", "Synchronize Horizontal Scrolling", ""),
        ("view.tabs.vertical", "Vertical Tabs", ""),
        ("view.tabs.pin", "Pin or Unpin Tab", ""),
        ("view.tabs.color", "Cycle Tab Color", ""),
        ("view.tabs.sort_name", "Sort Tabs by Name", ""),
        ("view.tabs.sort_path", "Sort Tabs by Path", ""),
        ("view.tabs.sort_descending", "Sort Tabs Descending", ""),
        ("view.tabs.move_left", "Move Tab Left", "Ctrl+Shift+PageUp"),
        ("view.tabs.move_right", "Move Tab Right", "Ctrl+Shift+PageDown"),
        ("view.tabs.previous", "Previous Tab", "Ctrl+PageUp"),
        ("view.tabs.next", "Next Tab", "Ctrl+PageDown"),
        ("view.tabs.mru", "Recent Document Switcher", "Ctrl+Tab"),
        ("view.tabs.closeAll", "Close All", ""),
        ("view.tabs.closeOthers", "Close Others", ""),
        ("view.tabs.closeLeft", "Close Tabs to the Left", ""),
        ("view.tabs.closeRight", "Close Tabs to the Right", ""),
    ] {
        let id = CommandId(id);
        let registered = registry.register(CommandSpec {
            id,
            title,
            category: "View",
            shortcut,
            action: Action::Contributed(id),
        });
        debug_assert!(registered.is_ok(), "duplicate command ID {id:?}");
        let _ = registry.set_presentation(
            id,
            CommandPresentation {
                menu_path: "View".into(),
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
    /// The bound document's id, even after its editor left the workspace.
    fn document(&self) -> u64 {
        match self {
            Self::Resident(_, snapshot) => snapshot.identity_token().0,
            Self::Paged(_, snapshot) => snapshot.identity_token().0,
        }
    }
    fn new(id: u64, editor: &bareline_app::workspace::WorkspaceEditor) -> Self {
        match editor {
            bareline_app::workspace::WorkspaceEditor::Resident(editor) => Self::Resident(id, editor.snapshot().clone()),
            bareline_app::workspace::WorkspaceEditor::Paged(editor) => Self::Paged(id, editor.snapshot().clone()),
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
            (Self::Resident(_, snapshot), bareline_app::workspace::WorkspaceEditor::Resident(editor)) => {
                snapshot.same_document(editor.snapshot())
            }
            (Self::Paged(_, snapshot), bareline_app::workspace::WorkspaceEditor::Paged(editor)) => {
                snapshot.same_document(editor.snapshot())
            }
            (Self::Resident(_, snapshot), WorkspaceEditor::Paged(editor)) => {
                snapshot.identity_token().0 == editor.snapshot().identity_token().0
            }
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
    /// The "all tabs" overflow list: strip order, opened by a click, so it stays
    /// open until a choice or dismissal instead of closing with Ctrl (UI-08).
    list: bool,
}
struct PendingViewScroll {
    selection_token: Option<u64>,
    state: ViewState,
    document: DocumentBinding,
}
/// A pane's published document state and view generation, with the pair it
/// replaced so the document's edit receipt maps to the view identity UIA saw.
#[derive(Clone, Copy)]
struct AccessibleViewSource {
    source: (u64, u64),
    generation: u64,
    previous: Option<((u64, u64), u64)>,
}
#[derive(Default)]
pub(super) struct ViewsRuntime {
    /// Banner band the secondary pane's own view reserves above its text,
    /// published by the shell before layout (UI-02).
    pub(super) secondary_banner_band: f32,
    /// The active pane's status groups as drawn this frame, before any fitting,
    /// so the shell footer fits full labels to its own width (UI-07). Empty
    /// when the active pane drew no status strip.
    pub(super) status_labels: Vec<String>,
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
    accessibility_sources: RefCell<BTreeMap<u64, AccessibleViewSource>>,
    accessibility_generation: Cell<u64>,
    pending_close: Option<usize>,
    controller: Option<ViewController>,
    primary: Option<ViewSnapshot>,
    pub(super) secondary: Option<WorkspaceEditor>,
    pub(super) retired: Vec<WorkspaceEditor>,
    pub(super) bounds: [Option<Rect>; 2],
    splitter: Option<Rect>,
    dragging: bool,
    queued: VecDeque<QueuedInput>,
    compare: bool,
    alignment: Option<bareline_app::views::AlignmentMap>,
    applied_spacers: [Option<Vec<(u64, u64)>>; 2],
    pending_sync: Option<(u32, u64)>,
    fold_target: Option<(u32, u64)>,
    /// Pages open this frame, set by the Shell before drawing, so the primary
    /// strip shows each as a closable tab (P3-6a / UX-54b, UI-05).
    open_pages: Vec<PageTab>,
    /// Pages left for a document keep their tab so they can be reselected (UI-09).
    parked_pages: Vec<PageTab>,
    /// "All tabs" buttons of overflowing strips (UI-08).
    tab_lists: Vec<(u32, Rect)>,
    /// Tabs a Close All/Others/Left/Right command still has to close, and the
    /// tab whose close is in flight (WSP-01).
    close_queue: VecDeque<u64>,
    close_current: Option<u64>,
}
/// The tabs of one strip that a Close All/Others/Left/Right command closes, in
/// strip order. Pinned tabs survive Close Others and Close to the Left/Right.
fn close_targets(tabs: &[(u64, bool)], active: Option<u64>, id: &str) -> Vec<u64> {
    let position = active.and_then(|active| tabs.iter().position(|(tab, _)| *tab == active));
    tabs.iter()
        .enumerate()
        .filter(|(index, (tab, pinned))| match id {
            "view.tabs.closeAll" => true,
            "view.tabs.closeOthers" => position.is_some() && Some(*tab) != active && !*pinned,
            "view.tabs.closeLeft" => position.is_some_and(|position| *index < position) && !*pinned,
            "view.tabs.closeRight" => position.is_some_and(|position| *index > position) && !*pinned,
            _ => false,
        })
        .map(|(_, (tab, _))| *tab)
        .collect()
}
const CLOSE_MULTIPLE_IDS: [&str; 4] = [
    "view.tabs.closeAll",
    "view.tabs.closeOthers",
    "view.tabs.closeLeft",
    "view.tabs.closeRight",
];
/// Settings and Extensions open as tabs in the primary strip (UI-05).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PageTab {
    Settings,
    Extensions,
}
impl PageTab {
    const ALL: [PageTab; 2] = [PageTab::Settings, PageTab::Extensions];
    /// Reserved tab ids. Real tab ids are small counters, so these never collide
    /// with a document tab, and `access_tab_id` rejects them.
    fn tab_id(self) -> u64 {
        match self {
            PageTab::Settings => SETTINGS_TAB_ID,
            PageTab::Extensions => EXTENSIONS_TAB_ID,
        }
    }
    fn from_tab_id(id: u64) -> Option<Self> {
        Self::ALL.into_iter().find(|page| page.tab_id() == id)
    }
    fn label(self) -> &'static str {
        match self {
            PageTab::Settings => "⚙ Settings",
            PageTab::Extensions => "Extensions",
        }
    }
    /// The command that shows this page again.
    fn command(self) -> &'static str {
        match self {
            PageTab::Settings => "settings.open",
            PageTab::Extensions => "extensions.manage",
        }
    }
    /// The tab's name for screen readers, without the drawn glyph.
    fn accessible_name(self) -> &'static str {
        match self {
            PageTab::Settings => "Settings",
            PageTab::Extensions => "Extensions",
        }
    }
    /// The page tab's accessibility id; its close button is the next id (UI-08).
    fn access_id(self) -> u64 {
        match self {
            PageTab::Settings => ACCESS_PAGE_TAB_BASE,
            PageTab::Extensions => ACCESS_PAGE_TAB_BASE + 2,
        }
    }
}
const SETTINGS_TAB_ID: u64 = u64::MAX;
const EXTENSIONS_TAB_ID: u64 = u64::MAX - 1;
/// Room each page tab takes at the end of the primary strip.
const PAGE_TAB_STEP: f32 = 130.0;
/// Room for the previous, next and "all tabs" buttons of a horizontal strip.
const TAB_NAV_RESERVE: f32 = 72.0;
impl ViewsRuntime {
    /// Records which pages are open this frame; an open page is no longer parked.
    pub(super) fn set_open_pages(&mut self, settings: bool, extensions: bool) {
        self.open_pages = PageTab::ALL
            .into_iter()
            .filter(|page| match page {
                PageTab::Settings => settings,
                PageTab::Extensions => extensions,
            })
            .collect();
        let open = self.open_pages.clone();
        self.parked_pages.retain(|page| !open.contains(page));
    }
    /// Keeps a page's tab in the strip after the page view is hidden.
    pub(super) fn park_page(&mut self, page: PageTab) {
        if !self.parked_pages.contains(&page) {
            self.parked_pages.push(page);
        }
    }
    /// Page tabs shown in the primary strip, in a stable order.
    fn page_tabs(&self) -> Vec<PageTab> {
        PageTab::ALL
            .into_iter()
            .filter(|page| self.open_pages.contains(page) || self.parked_pages.contains(page))
            .collect()
    }
    /// A page tab's ×: closes the page and removes its tab.
    fn close_page(
        &mut self,
        page: PageTab,
        settings: &mut super::settings::SettingsRuntime,
        extensions: &mut super::extensions::ExtensionsRuntime,
    ) {
        match page {
            PageTab::Settings if settings.controller.open => settings.controller.dismiss(),
            PageTab::Settings => {}
            PageTab::Extensions => extensions.open = false,
        }
        self.parked_pages.retain(|parked| *parked != page);
        self.open_pages.retain(|open| *open != page);
    }
    /// Activating a document leaves any page view (UI-09). The page keeps its
    /// tab so it can be reselected, as a real tab would.
    fn leave_pages(
        &mut self,
        settings: &mut super::settings::SettingsRuntime,
        extensions: &mut super::extensions::ExtensionsRuntime,
    ) {
        if settings.controller.open {
            settings.controller.dismiss();
            self.park_page(PageTab::Settings);
        }
        if extensions.open {
            extensions.open = false;
            self.park_page(PageTab::Extensions);
        }
        self.open_pages.clear();
    }
    /// Routes a press at pane-local `local` in split `pane` to the error actions
    /// of a failed open shown there; `true` when that pane shows one (FIO-01).
    fn failed_open_pointer(&self, workspace: &mut Workspace, pane: usize, local: Point) -> bool {
        let index = if pane == 1 {
            self.secondary_index(workspace)
        } else {
            self.primary_index(workspace)
        };
        index.is_some_and(|index| workspace.failed_open_pointer(index, local))
    }
    /// A left press at pane-local `local` in split `pane`: a failed open in
    /// either pane has no text, so its Retry and large-file actions take the
    /// press (FIO-01); otherwise the pane's editor places the caret, once a
    /// paged view has a ready frame.
    fn press_pane(
        &mut self,
        workspace: &mut Workspace,
        pane: usize,
        local: Point,
        renderer: Option<&impl TextBackend>,
        extend: bool,
    ) {
        if self.failed_open_pointer(workspace, pane, local) {
            return;
        }
        let editor = if pane == 1 {
            self.secondary.as_mut()
        } else {
            self.primary_index(workspace)
                .and_then(|index| workspace.editors.get_mut(index))
        };
        if let (Some(editor), Some(renderer)) = (editor, renderer)
            && !matches!(&*editor, WorkspaceEditor::Paged(paged) if !paged.paged_frame_state().ready)
        {
            let _ = editor.click(renderer, local, extend);
        }
    }
    /// Insert toggles overwrite for the focused pane's document view, as in a
    /// single view (UI-07); paged views edit through bounded windows and stay
    /// in insert mode.
    fn toggle_overwrite(&mut self, workspace: &mut Workspace, active: usize) {
        if let Some(editor) = self.active_workspace_editor_mut(workspace, active)
            && !editor.paged()
        {
            let overwrite = !editor.viewport().overwrite;
            editor.viewport_mut().overwrite = overwrite;
        }
    }
    /// Opens the list of every tab in `pane`, with the active one selected (UI-08).
    fn open_tab_list(&mut self, pane: u32) {
        let Some(controller) = &self.controller else {
            return;
        };
        let ids: Vec<u64> = controller.pane_tabs(pane).map(|tab| tab.id).collect();
        let selected = controller
            .active_tab(pane)
            .and_then(|active| ids.iter().position(|id| *id == active))
            .unwrap_or(0);
        self.mru_popup = Some(MruPopup {
            ids,
            selected,
            bounds: Rect::default(),
            list: true,
        });
    }
    pub(super) fn find_horizontal_geometry(&self, width: f32) -> (f32, f32) {
        let vertical = self
            .controller
            .as_ref()
            .is_some_and(|controller| controller.vertical_tabs);
        let inset = if !self.open() && vertical {
            176.0f32.min(width * 0.4)
        } else {
            0.0
        };
        (inset, (width - inset).max(0.0))
    }
    #[cfg(test)]
    pub(super) fn test_set_vertical_tabs(&mut self, workspace: &Workspace, vertical: bool) {
        self.sync_documents(workspace);
        self.controller.as_mut().unwrap().vertical_tabs = vertical;
    }
    #[cfg(test)]
    pub(super) fn test_activate_different_secondary(
        &mut self,
        workspace: &mut Workspace,
        primary_index: usize,
        secondary_index: usize,
    ) {
        self.split(workspace, secondary_index, Orientation::Vertical);
        let primary_document = self
            .documents
            .iter()
            .find(|binding| binding.matches(&workspace.editors[primary_index]))
            .map(DocumentBinding::id)
            .unwrap();
        let primary = self
            .controller
            .as_ref()
            .unwrap()
            .tabs()
            .iter()
            .find(|tab| tab.document_id == primary_document && tab.view.split == 0)
            .unwrap()
            .id;
        self.controller.as_mut().unwrap().activate(primary).unwrap();
        self.install_views(workspace);
        let secondary = self.controller.as_ref().unwrap().active_tab(1).unwrap();
        self.controller.as_mut().unwrap().activate(secondary).unwrap();
        self.install_views(workspace);
        assert_eq!(self.pane(), 1);
    }

    fn draw_tab_strip(
        &mut self,
        workspace: &Workspace,
        pane: u32,
        bounds: Rect,
        vertical: bool,
        ops: &mut Vec<DrawOp>,
    ) {
        self.tab_strips[pane as usize] = Some(bounds);
        // Page tabs live only on the primary, horizontal top strip; reserve room
        // for them so document tabs never draw underneath.
        let pages = if !vertical && bounds.x == 0.0 && bounds.y == 0.0 {
            self.page_tabs()
        } else {
            Vec::new()
        };
        let page_open = pages.iter().any(|page| self.open_pages.contains(page));
        let Some(controller) = &self.controller else {
            return;
        };
        let tabs: Vec<_> = controller.pane_tabs(pane).cloned().collect();
        let extent = if vertical { bounds.height } else { bounds.width };
        let page_reserve = PAGE_TAB_STEP * pages.len() as f32;
        let nav_reserve = if vertical { 48.0 } else { TAB_NAV_RESERVE };
        let available = (extent - nav_reserve - page_reserve).max(0.0);
        // Tabs shrink to fit down to a minimum width; the rest stay reachable by
        // the scroll arrows and the list of all tabs (UI-08).
        let (step, count) = if vertical {
            (TAB_HEIGHT, (available / TAB_HEIGHT).floor().max(1.0) as usize)
        } else {
            (
                bareline_ui::controls::TabStrip::fit_width(available, tabs.len()),
                bareline_ui::controls::TabStrip::fit_count(available, tabs.len()),
            )
        };
        let mut start = self.tab_offset[pane as usize].min(tabs.len().saturating_sub(count));
        // The narrow pane strip must keep its displayed compare source visible,
        // even when the wider document strip could fit earlier inactive tabs.
        if self.compare && bounds.y > 0.0 {
            if let Some(active) = tabs.iter().position(|tab| controller.active_tab(pane) == Some(tab.id)) {
                if active < start {
                    start = active;
                } else if active >= start + count {
                    start = active + 1 - count;
                }
            }
        }
        self.tab_offset[pane as usize] = start;
        ops.push(DrawOp::Fill(bounds, workspace.theme.chrome));
        ops.push(DrawOp::PushClip(bounds));
        let titles = workspace.titles();
        for (row, tab) in tabs.iter().skip(start).take(count).enumerate() {
            let bounds = if vertical {
                rect(bounds.x, bounds.y + row as f32 * step, bounds.width, TAB_HEIGHT)
            } else {
                rect(bounds.x + row as f32 * step, bounds.y, step, TAB_HEIGHT)
            };
            // While a page tab is shown, no document tab reads as the active one.
            let selected = !page_open && controller.active_tab(pane) == Some(tab.id);
            let index = self.document_index(workspace, tab.document_id);
            let title = index
                .and_then(|index| titles.get(index))
                .map(String::as_str)
                .unwrap_or("Document");
            let dirty = index.is_some_and(|index| workspace.editors[index].dirty());
            // `titles()` already appends the unsaved marker; strip it so the tab
            // renderer is the single source of the dot (fixes UX-30 double dot).
            let title = title.strip_suffix(" •").unwrap_or(title);
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
            // Fit the title to the (possibly shrunk) tab, ending in an ellipsis.
            let room = (((bounds.width - 40.0) / 6.0).floor().max(3.0) as usize)
                .saturating_sub(usize::from(tab.pinned) * 2 + usize::from(dirty) * 2)
                .max(1);
            let mut short: String = title.chars().take(room).collect();
            if title.chars().count() > room {
                short.pop();
                short.push('…');
            }
            let label = format!(
                "{}{}{}",
                if tab.pinned { "◆ " } else { "" },
                short,
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
            let close = rect(bounds.x + bounds.width - 24.0, bounds.y, 24.0, bounds.height);
            text(ops, close.x + 6.0, close.y + 7.0, "×", 14.0, workspace.theme.muted);
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
        for (slot, page) in pages.into_iter().enumerate() {
            let shown = self.open_pages.contains(&page);
            let tab = rect(
                bounds.x + extent - TAB_NAV_RESERVE - page_reserve + slot as f32 * PAGE_TAB_STEP + 6.0,
                bounds.y,
                PAGE_TAB_STEP - 12.0,
                TAB_HEIGHT,
            );
            ops.push(DrawOp::Fill(
                tab,
                if shown {
                    workspace.theme.editor
                } else {
                    workspace.theme.chrome
                },
            ));
            ops.push(DrawOp::Stroke(tab, workspace.theme.border, 1.0));
            text(
                ops,
                tab.x + 10.0,
                tab.y + 8.0,
                page.label(),
                13.0,
                if shown {
                    workspace.theme.text
                } else {
                    workspace.theme.muted
                },
            );
            let close = rect(tab.x + tab.width - 24.0, tab.y, 24.0, tab.height);
            text(ops, close.x + 6.0, close.y + 7.0, "×", 14.0, workspace.theme.muted);
            if shown {
                ops.push(DrawOp::Fill(
                    rect(tab.x, tab.y + tab.height - 2.0, tab.width, 2.0),
                    workspace.theme.focus,
                ));
            }
            self.tab_hits.push(TabHit {
                id: page.tab_id(),
                pane,
                bounds: tab,
                close,
            });
        }
        // Only show the scroll arrows and the list of all tabs when the tabs
        // overflow the strip; when they all fit they are hidden (UX-39, UI-08).
        if tabs.len() > count {
            for (slot, label) in ["‹", "›", "▾"].into_iter().enumerate() {
                let nav = if vertical {
                    rect(
                        bounds.x + bounds.width * slot as f32 / 3.0,
                        bounds.y + bounds.height - 24.0,
                        bounds.width / 3.0,
                        24.0,
                    )
                } else {
                    rect(
                        bounds.x + bounds.width - TAB_NAV_RESERVE + slot as f32 * 24.0,
                        bounds.y,
                        24.0,
                        TAB_HEIGHT,
                    )
                };
                text(ops, nav.x + 8.0, nav.y + 6.0, label, 14.0, workspace.theme.text);
                if slot == 2 {
                    self.tab_lists.push((pane, nav));
                } else {
                    self.tab_nav.push((pane, slot == 1, nav));
                }
            }
        }
        ops.push(DrawOp::PopClip);
    }
    fn draw_mru(&mut self, workspace: &Workspace, width: f32, height: f32, ops: &mut Vec<DrawOp>) {
        let Some(popup) = &self.mru_popup else {
            return;
        };
        let ids = popup.ids.clone();
        let selected = popup.selected;
        let rows_height = (height - 80.0).clamp(0.0, 12.0 * TAB_HEIGHT);
        // The list of all tabs drops down under the strip's right-hand buttons.
        let bounds = if popup.list {
            rect(
                (width - 360.0).max(0.0),
                TAB_HEIGHT,
                width.min(360.0),
                rows_height.min(ids.len().max(1) as f32 * TAB_HEIGHT),
            )
        } else {
            rect(
                (width - 360.0).max(0.0) / 2.0,
                TAB_HEIGHT + 12.0,
                width.min(360.0),
                rows_height,
            )
        };
        self.mru_popup.as_mut().unwrap().bounds = bounds;
        ops.push(DrawOp::Fill(bounds, workspace.theme.chrome));
        ops.push(DrawOp::Stroke(bounds, workspace.theme.border, 1.0));
        ops.push(DrawOp::PushClip(bounds));
        let titles = workspace.titles();
        let visible = mru_visible_rows(bounds);
        let start = selected.saturating_sub(visible.saturating_sub(1));
        for (row, id) in ids.iter().skip(start).take(visible).enumerate() {
            let row_bounds = rect(bounds.x, bounds.y + row as f32 * TAB_HEIGHT, bounds.width, TAB_HEIGHT);
            // The row selection pair, not the interactive border (the text
            // colour in high contrast), marks the selected row (A11Y-01).
            let is_selected = row + start == selected;
            if is_selected {
                bareline_ui::widgets::paint_selected_row(row_bounds, workspace.theme.widgets(), ops);
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
                if is_selected {
                    workspace.theme.selection_row_text
                } else {
                    workspace.theme.text
                },
            );
        }
        ops.push(DrawOp::PopClip);
    }
    fn document_index(&self, workspace: &Workspace, id: u64) -> Option<usize> {
        let binding = self.documents.iter().find(|binding| binding.id() == id)?;
        workspace.editors.iter().position(|editor| binding.matches(editor))
    }
    fn has_tab(&self, id: u64) -> bool {
        self.controller
            .as_ref()
            .is_some_and(|controller| controller.tab(id).is_some())
    }
    fn tab_index(&self, workspace: &Workspace, id: u64) -> Option<usize> {
        self.document_index(workspace, self.controller.as_ref()?.tab(id)?.document_id)
    }
    /// A closed tab restored from disk is a new document. Its closed (or still
    /// loading) tab follows it, keeping pin, position, color and view (WSP-05).
    pub(super) fn rebind_closed(&mut self, previous: u64, editor: &WorkspaceEditor) {
        if let Some(binding) = self
            .documents
            .iter_mut()
            .chain(self.closed_documents.iter_mut().map(|(binding, _)| binding))
            .find(|binding| binding.document() == previous)
        {
            *binding = DocumentBinding::new(binding.id(), editor);
        }
    }
    fn sync_documents(&mut self, workspace: &Workspace) {
        // Promotion preserves logical identity but changes the actor facade. Rebind
        // existing linked panes even though their stable tab IDs did not change.
        for binding in &mut self.documents {
            let DocumentBinding::Resident(id, source) = binding else {
                continue;
            };
            let Some(WorkspaceEditor::Paged(promoted)) = workspace.editors.iter().find(|editor| matches!(editor,WorkspaceEditor::Paged(paged) if paged.snapshot().identity_token().0 == source.identity_token().0)) else { continue; };
            if self.secondary.as_ref().is_some_and(
                |peer| matches!(peer,WorkspaceEditor::Resident(resident) if resident.snapshot().same_document(source)),
            ) {
                let old = self.secondary.as_ref().unwrap();
                let state = workspace_view_state(old);
                match promoted.clone_view() {
                    Ok(mut peer) => {
                        old.copy_presentation_to(peer.viewport_mut());
                        let old = self.secondary.replace(WorkspaceEditor::Paged(peer)).unwrap();
                        self.retired.push(old);
                        self.pending_restore[1] = Some(state);
                        self.pending_view_scroll[1] = None;
                        self.applied_spacers[1] = None;
                    }
                    Err(_) => continue, // Retry without discarding the linked view.
                }
            }
            if self
                .primary
                .as_ref()
                .is_some_and(|primary| primary.same_document(source))
            {
                self.primary = Some(promoted.viewport().snapshot().clone());
                self.applied_spacers[0] = None;
            }
            *binding = DocumentBinding::Paged(*id, promoted.snapshot().clone());
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
        self.rebind_finished_opens(workspace);
        if self.controller.is_none() {
            self.controller = ViewController::new(Vec::new(), None).ok();
        }
        if let Some(controller) = &self.controller {
            for binding in &self.documents {
                if !workspace.editors.iter().any(|editor| binding.matches(editor)) {
                    let tabs = controller
                        .tabs()
                        .iter()
                        .enumerate()
                        .filter(|(_, tab)| tab.document_id == binding.id())
                        .map(|(position, tab)| (position, tab.clone(), controller.tab_colors.get(&tab.id).copied()))
                        .collect();
                    self.closed_documents.push_back((binding.clone(), tabs));
                    while self.closed_documents.len() > 20 {
                        self.closed_documents.pop_front();
                    }
                }
            }
        }
        self.documents
            .retain(|binding| workspace.editors.iter().any(|editor| binding.matches(editor)));
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
                self.documents.push(DocumentBinding::new(self.next_document, editor));
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
            let live_tabs: std::collections::BTreeSet<_> = controller.tabs().iter().map(|tab| tab.id).collect();
            self.accessibility_sources
                .borrow_mut()
                .retain(|tab, _| live_tabs.contains(tab));
        }
        self.documents.sort_by_key(|binding| {
            workspace
                .editors
                .iter()
                .position(|editor| binding.matches(editor))
                .unwrap_or(usize::MAX)
        });
    }
    /// An open that finishes in place (loading, failed or paged fallback), a
    /// Reload or Interpret As, or a recovered document adopted for its loading
    /// tab gives the tab a new document. The tab keeps its position, pin and
    /// colour for that document instead of closing and reappearing at the end
    /// (PED-23); its view starts from the new document's (WSP-11).
    /// A duplicate open resolves to a document that already has a tab, so its
    /// own tab closes as before.
    fn rebind_finished_opens(&mut self, workspace: &Workspace) {
        for position in 0..self.documents.len() {
            let old = &self.documents[position];
            if workspace.editors.iter().any(|editor| old.matches(editor)) {
                continue;
            }
            let replacement = workspace.replacement_document(old.document());
            if replacement == old.document() {
                continue;
            }
            let Some(editor) = workspace
                .editors
                .iter()
                .find(|editor| editor.document_identity().0 == replacement)
            else {
                continue;
            };
            if self.documents.iter().any(|binding| binding.matches(editor)) {
                continue;
            }
            let id = old.id();
            self.documents[position] = DocumentBinding::new(id, editor);
            let tabs: Vec<u64> = self.controller.as_ref().map_or_else(Vec::new, |controller| {
                controller
                    .tabs()
                    .iter()
                    .filter(|tab| tab.document_id == id)
                    .map(|tab| tab.id)
                    .collect()
            });
            if let Some(controller) = &mut self.controller {
                // A stored view belonged to the placeholder's text, not this one.
                for tab in &tabs {
                    let _ = controller.set_view_state(*tab, workspace_view_state(editor));
                }
            }
            for pane in 0..2 {
                if !self.loaded_tabs[pane].is_some_and(|tab| tabs.contains(&tab)) {
                    continue;
                }
                self.pending_restore[pane] = None;
                self.pending_view_scroll[pane] = None;
                self.applied_spacers[pane] = None;
                if pane == 0 {
                    self.primary = Some(editor.snapshot().clone());
                    continue;
                }
                let peer = match editor {
                    WorkspaceEditor::Resident(editor) => Ok(WorkspaceEditor::Resident(editor.clone_view())),
                    WorkspaceEditor::Paged(editor) => editor.clone_view().map(WorkspaceEditor::Paged),
                };
                if let Ok(peer) = peer
                    && let Some(old) = self.secondary.replace(peer)
                {
                    self.retired.push(old);
                }
            }
        }
    }
    fn save_view_states(&self, workspace: &Workspace, controller: &mut ViewController) {
        for pane in 0..2 {
            let Some(id) = self.loaded_tabs[pane] else {
                continue;
            };
            let editor = if pane == 1 {
                self.secondary.as_ref()
            } else {
                self.primary_index(workspace).map(|index| &workspace.editors[index])
            };
            if let Some(editor) = editor {
                let mut state = self.pending_restore[pane]
                    .as_ref()
                    .or(self.pending_view_scroll[pane].as_ref().map(|pending| &pending.state))
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
        for (pane, id) in ids.into_iter().enumerate() {
            let stale_secondary = pane == 1 && id.is_none() && self.secondary.is_some();
            if id == self.loaded_tabs[pane] && !stale_secondary {
                continue;
            }
            self.applied_spacers[pane] = None;
            let tab = id.and_then(|id| self.controller.as_ref().unwrap().tab(id)).cloned();
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
                        if let Err(error) = restore_workspace_view(&mut workspace.editors[index], &tab.view) {
                            workspace.editors[index].viewport_mut().error = Some(error);
                        }
                    }
                } else {
                    self.primary = None;
                }
            } else {
                self.pending_restore[pane] = None;
                self.pending_view_scroll[pane] = None;
                self.return_secondary_history(workspace);
                if let Some(old) = self.secondary.take() {
                    self.retired.push(old);
                }
                if let (Some(index), Some(tab)) = (index, tab) {
                    let peer = match &workspace.editors[index] {
                        WorkspaceEditor::Resident(editor) => Ok(WorkspaceEditor::Resident(editor.clone_view())),
                        WorkspaceEditor::Paged(editor) => editor.clone_view().map(WorkspaceEditor::Paged),
                    };
                    match peer {
                        Ok(mut peer) => {
                            if peer.paged() {
                                self.pending_restore[pane] = Some(tab.view);
                            } else {
                                if let Err(error) = restore_workspace_view(&mut peer, &tab.view) {
                                    peer.viewport_mut().error = Some(error);
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
    /// Activates tab `id`; `false` when it could not change (views busy, or the
    /// tab is gone), so callers leave Settings/Extensions only on success.
    fn select_tab(&mut self, workspace: &mut Workspace, app: &mut App, id: u64) -> bool {
        if self.busy(workspace) {
            workspace.message = Some("Wait for pending edits before changing tabs.".into());
            return false;
        }
        self.save_current(workspace);
        let activated = self
            .controller
            .as_mut()
            .is_some_and(|controller| controller.activate(id).is_ok());
        if activated {
            self.install_views(workspace);
            if let Some(index) = self.tab_index(workspace, id) {
                app.active = index;
            }
            self.bind_find_to_active(workspace);
            if !self.tab_hits.iter().any(|hit| hit.id == id)
                && let Some(controller) = &self.controller
            {
                let pane = controller.active_pane();
                self.tab_offset[pane as usize] = controller.pane_tabs(pane).position(|tab| tab.id == id).unwrap_or(0);
            }
        }
        activated
    }
    pub(super) fn active_editor<'a>(
        &'a self,
        workspace: &'a Workspace,
        fallback: usize,
    ) -> Option<&'a SharedEditorView> {
        if self.pane() == 1 {
            self.secondary.as_ref().map(|editor| editor.viewport())
        } else {
            workspace.editors.get(fallback).map(|editor| editor.viewport())
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
    pub(super) fn active_selection(
        &self,
        workspace: &Workspace,
        fallback: usize,
    ) -> Option<std::ops::Range<bareline_document::TextOffset>> {
        let editor = self.active_workspace_editor(workspace, fallback)?;
        if editor.busy() || matches!(editor, WorkspaceEditor::Paged(paged) if !paged.paged_frame_state().ready) {
            return None;
        }
        let (anchor, caret) = match editor {
            WorkspaceEditor::Paged(paged) => paged.global_selection(),
            WorkspaceEditor::Resident(editor) => (
                bareline_document::TextOffset(editor.selection.anchor),
                bareline_document::TextOffset(editor.selection.caret),
            ),
        };
        Some(anchor.min(caret)..anchor.max(caret))
    }
    pub(super) fn pane_workspace_editor<'a>(
        &'a self,
        workspace: &'a Workspace,
        fallback: usize,
        pane: usize,
    ) -> Option<&'a WorkspaceEditor> {
        match pane {
            0 => self
                .primary_index(workspace)
                .or(Some(fallback))
                .and_then(|index| workspace.editors.get(index)),
            1 if self.open() => self.secondary.as_ref(),
            _ => None,
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
            if editor.snapshot().same_document(snapshot) && editor.snapshot().revision == snapshot.revision {
                editor.set_known_folds(folds, level, partial);
                applied = true;
            }
        }
        if applied && pane == 1 {
            if let (Some(index), Some(WorkspaceEditor::Resident(peer))) = (index, &self.secondary) {
                workspace.editors[index].viewport_mut().sync_fold_metadata_from(peer);
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
    pub(super) fn pane_workspace_editor_mut<'a>(
        &'a mut self,
        workspace: &'a mut Workspace,
        fallback: usize,
        pane: usize,
    ) -> Option<&'a mut WorkspaceEditor> {
        match pane {
            0 => {
                let index = self.primary_index(workspace).unwrap_or(fallback);
                workspace.editors.get_mut(index)
            }
            1 if self.open() => self.secondary.as_mut(),
            _ => None,
        }
    }
    pub(super) fn active_editor_mut<'a>(
        &'a mut self,
        workspace: &'a mut Workspace,
        fallback: usize,
    ) -> Option<&'a mut SharedEditorView> {
        if self.pane() == 1 {
            self.secondary.as_mut().map(|editor| editor.viewport_mut())
        } else {
            workspace.editors.get_mut(fallback).map(|editor| editor.viewport_mut())
        }
    }
    pub(super) fn history_available(&self, workspace: Option<&Workspace>, active: usize, undo: bool) -> bool {
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

    pub(super) fn annotate_context(
        &self,
        context: &mut bareline_commands::CommandContext,
        tabs: &[String],
        active: usize,
    ) {
        use bareline_commands::CommandState;
        // The Window menu is a live list of open documents with a radio dot on
        // the active one; slots past the open count drop out of the menu.
        for (i, id) in WINDOW_IDS.into_iter().enumerate() {
            match tabs.get(i) {
                Some(title) => {
                    let label = if i < 9 {
                        format!("&{}  {}", i + 1, title)
                    } else {
                        format!("{}  {}", i + 1, title)
                    };
                    context.states.insert(
                        CommandId(id),
                        CommandState {
                            label: Some(label),
                            checked: i == active,
                            radio: true,
                            ..Default::default()
                        },
                    );
                }
                None => {
                    context
                        .states
                        .insert(CommandId(id), CommandState::not_applicable("No document in this slot"));
                }
            }
        }
        for id in ["view.tabs.next", "view.tabs.previous"] {
            if tabs.len() < 2 {
                context
                    .states
                    .insert(CommandId(id), CommandState::disabled("Open another document first"));
            }
        }
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
            context.states.entry(CommandId("view.tabs.pin")).or_default().checked = controller
                .active_tab(controller.active_pane())
                .and_then(|id| controller.tab(id))
                .is_some_and(|tab| tab.pinned);
            let pane = controller.active_pane();
            let strip: Vec<(u64, bool)> = controller.pane_tabs(pane).map(|tab| (tab.id, tab.pinned)).collect();
            for id in CLOSE_MULTIPLE_IDS {
                if close_targets(&strip, controller.active_tab(pane), id).is_empty() {
                    context
                        .states
                        .insert(CommandId(id), CommandState::disabled("No tabs to close"));
                }
            }
        }
        if self.close_current.is_some() || !self.close_queue.is_empty() {
            for id in CLOSE_MULTIPLE_IDS {
                context
                    .states
                    .insert(CommandId(id), CommandState::disabled("Tabs are already closing"));
            }
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
                    && self
                        .controller
                        .as_ref()
                        .is_some_and(|controller| controller.orientation == Orientation::Horizontal),
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
    pub(super) fn compare_pair(&mut self, workspace: &mut Workspace, left: usize, right: usize) -> bool {
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
        if self.busy(workspace) {
            workspace.message = Some("Wait for pending edits before closing the comparison.".into());
            return;
        }
        self.compare = false;
        self.alignment = None;
        self.collapse(workspace, false);
    }
    pub(super) fn compare_geometry(&self) -> [Option<Rect>; 2] {
        self.bounds
    }
    pub(super) fn compare_selections(&self, workspace: &Workspace) -> Option<[bareline_editor_surface::Selection; 2]> {
        let primary = workspace.editors.get(self.primary_index(workspace)?)?;
        let secondary = self.secondary.as_ref()?;
        let selection = |editor: &WorkspaceEditor| match editor {
            WorkspaceEditor::Paged(paged) => {
                let (anchor, caret) = paged.global_selection();
                bareline_editor_surface::Selection {
                    anchor: anchor.0,
                    caret: caret.0,
                }
            }
            editor => editor.viewport().selection,
        };
        Some([selection(primary), selection(secondary)])
    }
    pub(super) fn compare_source_layout_range(
        &self,
        workspace: &Workspace,
        side: usize,
        layout: bareline_renderer::LayoutId,
    ) -> Option<std::ops::Range<bareline_document::TextOffset>> {
        let editor = if side == 0 {
            workspace.editors.get(self.primary_index(workspace)?)?
        } else if side == 1 {
            self.secondary.as_ref()?
        } else {
            return None;
        };
        let local = editor.layout_range(layout)?;
        match editor {
            WorkspaceEditor::Resident(_) => Some(local),
            WorkspaceEditor::Paged(paged) => {
                use bareline_editor_surface::paged_view::SourceAffinity;
                let start = paged.source_offset(local.start, SourceAffinity::After)?;
                let end = paged.source_offset(local.end, SourceAffinity::Before)?;
                (end.0.checked_sub(start.0) == Some(local.end.0 - local.start.0)).then_some(start..end)
            }
        }
    }
    pub(super) fn set_compare_alignment(&mut self, alignment: Option<bareline_app::views::AlignmentMap>) {
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
            (left, first.and_then(|index| workspace.editors.get_mut(index))),
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
                while !editor.snapshot().is_boundary(bareline_document::TextOffset(offset)) {
                    offset -= 1;
                }
                editor.viewport_mut().selection.anchor = offset;
                editor.viewport_mut().selection.caret = offset;
                let scroll_y = editor
                    .snapshot()
                    .line_at(bareline_document::TextOffset(offset))
                    .unwrap_or(0) as f64
                    * 19.2;
                editor.viewport_mut().scroll_y = scroll_y;
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
                if !self.documents.iter().any(|binding| binding.id() == tab.document_id) {
                    self.documents.push(DocumentBinding::new(tab.document_id, editor));
                }
            }
        }
        self.next_document = manifest.documents.iter().map(|document| document.id).max().unwrap_or(0);
        self.controller = Some(controller);
        // A restore can reuse persisted tab IDs for different document actors.
        // Retire every qualified reader while preserving the monotonic serial.
        self.accessibility_sources.borrow_mut().clear();
        self.loaded_tabs = [None, None];
        self.install_views(workspace);
        if let Some(id) = manifest.active_tab.and_then(|id| self.tab_index(workspace, id)) {
            app.active = id;
        }
        self.bind_find_to_active(workspace);
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
                let document_id = manifest.tabs.iter().find(|tab| tab.id == *tab_id)?.document_id;
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
        self.secondary.as_ref().and_then(|editor| editor.blink_deadline())
    }
    pub(super) fn tick_secondary_caret_blink(&mut self, now: Instant) -> bool {
        self.secondary
            .as_mut()
            .is_some_and(|editor| editor.tick_caret_blink(now))
    }
    pub(super) fn pending_edits(&self) -> bool {
        !self.queued.is_empty() || self.secondary.as_ref().is_some_and(WorkspaceEditor::busy)
    }
    pub(super) fn take_ordered_receipts(&mut self) -> Vec<bareline_editor_surface::power::consumer::OrderedReceipt> {
        self.secondary
            .as_mut()
            .map(|editor| editor.take_ordered_receipts())
            .unwrap_or_default()
    }
    pub(super) fn open(&self) -> bool {
        self.secondary.is_some() && self.controller.as_ref().is_some_and(|c| c.split)
    }
    fn close_split(&mut self, workspace: &mut Workspace) {
        if !self.busy(workspace) {
            self.collapse(workspace, self.pane() == 1);
        }
    }
    /// The active pane's verified syntax for completion and parameter hints. A
    /// provisional stand-in carried through an edit is not offered: it grows
    /// spans over typed text (text typed after a string reads as string).
    pub(super) fn active_syntax_result<'a>(
        &'a self,
        workspace: &'a Workspace,
    ) -> Option<&'a bareline_syntax::SyntaxResult> {
        let result = if self.secondary.is_none() {
            workspace.syntax_result()
        } else {
            let pane = self.pane() as usize;
            let editor = if pane == 1 {
                self.secondary.as_ref()?
            } else {
                workspace.editors.get(self.primary_index(workspace)?)?
            };
            self.styling[pane].syntax_view(editor).result
        };
        result.filter(|result| result.status == bareline_syntax::Status::Complete)
    }
    pub(super) fn pane_token(&self, pane: usize) -> Option<u64> {
        self.loaded_tabs.get(pane).copied().flatten()
    }

    pub(super) fn accessibility_source_identity(&self, pane: usize, editor: &WorkspaceEditor) -> Option<(u64, u64)> {
        let tab = self.pane_token(pane)?;
        let source = bareline_app::accessibility::source_identity(editor);
        let mut sources = self.accessibility_sources.borrow_mut();
        let current = sources.get(&tab).copied();
        let generation = if let Some(current) = current
            && current.source == source
        {
            current.generation
        } else {
            let generation = self
                .accessibility_generation
                .get()
                .checked_add(1)
                .expect("editor accessibility generation exhausted");
            self.accessibility_generation.set(generation);
            sources.insert(
                tab,
                AccessibleViewSource {
                    source,
                    generation,
                    previous: current.map(|current| (current.source, current.generation)),
                },
            );
            generation
        };
        Some((super::accessibility::editor_provider_id(tab), generation))
    }
    /// The document state and view identity the pane published before its
    /// current generation, as `(document identity, view identity)`.
    pub(super) fn accessibility_previous_source(&self, pane: usize) -> Option<((u64, u64), (u64, u64))> {
        let tab = self.pane_token(pane)?;
        let (source, generation) = self.accessibility_sources.borrow().get(&tab)?.previous?;
        Some((source, (super::accessibility::editor_provider_id(tab), generation)))
    }
    pub(super) fn pane_document_index(&self, workspace: &Workspace, pane: usize) -> Option<usize> {
        self.pane_token(pane).and_then(|tab| self.tab_index(workspace, tab))
    }
    pub(super) fn pane(&self) -> u32 {
        self.controller.as_ref().map_or(0, ViewController::active_pane)
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
        self.primary.as_ref().and_then(|s| Self::index_of(workspace, s))
    }
    /// The unfitted status groups of document `active` when `drawn` holds its
    /// status strip this frame; empty when something else was painted (UI-07).
    fn drawn_status_labels(workspace: &Workspace, active: usize, drawn: &[DrawOp], height: f32) -> Vec<String> {
        let shown = drawn
            .iter()
            .filter(|op| matches!(op, DrawOp::Text { origin, .. } if origin.y == height - 20.0))
            .count();
        workspace
            .editors
            .get(active)
            .map(|editor| &editor.viewport().status_labels)
            .filter(|labels| !labels.is_empty() && labels.len() == shown)
            .cloned()
            .unwrap_or_default()
    }
    pub(super) fn secondary_index(&self, workspace: &Workspace) -> Option<usize> {
        self.loaded_tabs[1].and_then(|id| self.tab_index(workspace, id))
    }
    pub(super) fn busy(&self, workspace: &Workspace) -> bool {
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
                        changed |= primary.refresh_linked_peer(secondary);
                    } else if primary.snapshot().revision.0 > secondary.snapshot().revision.0 {
                        changed |= secondary.refresh_linked_peer(primary);
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
            peer.viewport_mut().theme = workspace.editors[index].viewport().theme;
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
                            editor.viewport_mut().error =
                                Some("The document changed while its view was being restored.".into());
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
                                Err(error) => {
                                    editor.viewport_mut().error = Some(error);
                                    changed = true;
                                }
                            }
                        }
                    }
                    if let Some(state) = self.pending_restore[pane].take() {
                        let document = DocumentBinding::new(0, editor);
                        match restore_workspace_view(editor, &state) {
                            Ok(()) if editor.paged() => {
                                self.pending_view_scroll[pane] = Some(PendingViewScroll {
                                    state,
                                    document,
                                    selection_token: None,
                                })
                            }
                            Ok(()) => {}
                            Err(error) => editor.viewport_mut().error = Some(error),
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
            self.move_resident_history_to(workspace, queued.pane);
            if queued.pane == 0 {
                if let Some(index) = self.primary_index(workspace) {
                    if queued.document.matches(&workspace.editors[index]) {
                        workspace.editors[index].enqueue(queued.input);
                    } else {
                        workspace.message = Some("The view changed before queued input could be applied.".into());
                    }
                }
            } else if let Some(editor) = &mut self.secondary {
                if queued.document.matches(editor) {
                    editor.enqueue(queued.input);
                } else {
                    workspace.message = Some("The view changed before queued input could be applied.".into());
                }
            }
            changed = true;
        }
        changed |= self.flush_sync_scroll(workspace);
        changed
    }
    /// The Tab shortcut inserts at empty carets; explicit Indent and selected
    /// lines continue through command dispatch. Use the shared input queue so
    /// resident and paged panes keep their normal history and synchronization.
    pub(super) fn insert_tab_at_carets(&mut self, workspace: &mut Workspace, fallback: usize) -> bool {
        let Some(editor) = self.active_workspace_editor(workspace, fallback) else {
            return false;
        };
        if editor.viewport().active_rectangle().is_some()
            || editor
                .selection_set()
                .selections
                .iter()
                .any(|selection| selection.anchor != selection.caret)
        {
            return false;
        }
        if self.open() {
            self.input(workspace, self.pane(), Input::Insert("\t".into()));
        } else if let Some(editor) = workspace.editors.get_mut(fallback) {
            editor.enqueue(Input::Insert("\t".into()));
        }
        true
    }

    fn input(&mut self, workspace: &mut Workspace, pane: u32, input: Input) {
        self.pump(workspace);
        self.move_resident_history_to(workspace, pane);
        let document = if pane == 1 {
            self.secondary.as_ref().map(|editor| DocumentBinding::new(0, editor))
        } else {
            self.primary_index(workspace)
                .map(|i| DocumentBinding::new(0, &workspace.editors[i]))
        };
        let Some(document) = document else {
            return;
        };
        if self.queued.len() >= 256 {
            workspace.message = Some("Split-view input queue is full; wait for the pending edit.".into());
            return;
        }
        self.queued.push_back(QueuedInput { pane, document, input });
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
            .or_else(|| controller.tabs().iter().find(|tab| tab.document_id == document))
            .map(|tab| tab.id)
            .unwrap();
        let _ = controller.activate(id);
        if !controller.split {
            let _ = controller.clone_to_other(id);
        }
        controller.orientation = orientation;
        controller.split = true;
        self.install_views(workspace);
        let pane = self.pane();
        self.move_resident_history_to(workspace, pane);
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
        self.move_resident_history_to(workspace, 0);
        if let Some(controller) = &mut self.controller {
            if keep_secondary && let Some(id) = controller.active_tab(1) {
                let _ = controller.activate(id);
            }
            controller.collapse();
        }
        self.loaded_tabs = [None, None];
        self.install_views(workspace);
        self.bounds = [None, None];
        self.splitter = None;
        self.dragging = false;
        self.bind_find_to_active(workspace);
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
        self.bind_find_to_active(workspace);
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
        self.bind_find_to_active(workspace);
    }
    pub(super) fn activate_watch_pane(&mut self, workspace: &mut Workspace, app: &mut App, pane: u32) -> bool {
        if pane > 1 || (pane == 1 && self.secondary.is_none()) {
            return false;
        }
        self.activate(workspace, app, pane);
        true
    }
    pub(super) fn activate(&mut self, workspace: &mut Workspace, app: &mut App, pane: u32) {
        self.move_resident_history_to(workspace, pane);
        if let Some(controller) = &mut self.controller
            && let Some(id) = controller.active_tab(pane)
        {
            let _ = controller.activate(id);
        }
        if let Some(index) = if pane == 0 {
            self.primary_index(workspace)
        } else {
            self.secondary_index(workspace)
        } {
            app.active = index;
        }
        self.bind_find_to_active(workspace);
    }
    fn move_resident_history_to(&mut self, workspace: &mut Workspace, pane: u32) {
        let Some(index) = self.secondary_index(workspace) else {
            return;
        };
        let Some(secondary) = self.secondary.as_mut() else {
            return;
        };
        let primary = &mut workspace.editors[index];
        match (primary, secondary) {
            (WorkspaceEditor::Resident(primary), WorkspaceEditor::Resident(secondary)) if pane == 0 => {
                let _ = primary.take_peer_history(secondary);
            }
            (WorkspaceEditor::Resident(primary), WorkspaceEditor::Resident(secondary)) if pane == 1 => {
                let _ = secondary.take_peer_history(primary);
            }
            _ => {}
        }
    }
    fn return_secondary_history(&mut self, workspace: &mut Workspace) {
        let Some(secondary) = self.secondary.as_mut() else {
            return;
        };
        let Some(index) = Self::index_of(workspace, secondary.snapshot()) else {
            return;
        };
        if let (WorkspaceEditor::Resident(primary), WorkspaceEditor::Resident(secondary)) =
            (&mut workspace.editors[index], secondary)
        {
            let _ = primary.take_peer_history(secondary);
        }
    }
    pub(super) fn accessibility_activate_editor(&mut self, workspace: &mut Workspace, app: &mut App, pane: u32) {
        self.accessibility_focus = None;
        self.activate(workspace, app, pane);
    }
    fn bind_find_to_active(&self, workspace: &mut Workspace) {
        enum Source {
            Resident(bareline_document::DocumentSnapshot),
            Paged(bareline_document::paged::PagedSnapshot),
        }
        let source = if self.pane() == 1 {
            self.secondary.as_ref()
        } else {
            self.primary_index(workspace)
                .and_then(|index| workspace.editors.get(index))
        }
        .map(|editor| match editor {
            WorkspaceEditor::Resident(editor) => Source::Resident(editor.snapshot().clone()),
            WorkspaceEditor::Paged(editor) => Source::Paged(editor.snapshot().clone()),
        });
        match source {
            Some(Source::Resident(source)) => workspace.bind_find_resident(&source),
            Some(Source::Paged(source)) => workspace.bind_find_paged(&source),
            None => workspace.clear_find_source(),
        }
    }
    fn refresh_find_to_active(&self, workspace: &mut Workspace, notify: std::sync::Arc<dyn Fn() + Send + Sync>) {
        enum Source {
            Resident(bareline_document::DocumentSnapshot),
            Paged(bareline_editor_surface::paged_view::PagedReadHandle),
        }
        let source = if self.pane() == 1 {
            self.secondary.as_ref()
        } else {
            self.primary_index(workspace)
                .and_then(|index| workspace.editors.get(index))
        }
        .map(|editor| match editor {
            WorkspaceEditor::Resident(editor) => Source::Resident(editor.snapshot().clone()),
            WorkspaceEditor::Paged(editor) => Source::Paged(editor.read_handle()),
        });
        match source {
            Some(Source::Resident(source)) => {
                workspace.bind_find_resident(&source);
                workspace.find.refresh(&source, notify);
            }
            Some(Source::Paged(handle)) => {
                workspace.bind_find_paged(handle.snapshot());
                workspace.find.refresh_paged(handle, notify);
            }
            None => workspace.clear_find_source(),
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
        let _ = self
            .controller
            .as_mut()
            .unwrap()
            .close(id, workspace.editors[index].dirty(), false);
        self.accessibility_sources.borrow_mut().remove(&id);
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
        self.bind_find_to_active(workspace);
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
                bareline_editor_surface::paged_view::GlobalScrollPosition::Ready(line, fraction, x) => {
                    (line, fraction, x)
                }
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
                        if let Err(error) =
                            editor.request_global_scroll(update.position.line, update.position.fraction, x)
                        {
                            editor.error = Some(error);
                        }
                    }
                    editor => editor.set_logical_scroll(update.position.line, update.position.fraction, x),
                }
            } else {
                target.scroll_horizontal(x - old.2);
            }
        }
        true
    }
    /// Pump the split-view workers and follow the active document's tab
    /// selection. Rendering must not mutate state (ARCH-13), so this runs from
    /// the frame loop immediately before [`Self::draw`] rather than inside it.
    pub(super) fn sync(&mut self, workspace: &mut Workspace, app: &mut App) {
        self.pump(workspace);
        let desired = self.controller.as_ref().and_then(|controller| {
            controller
                .pane_tabs(controller.active_pane())
                .find(|tab| self.document_index(workspace, tab.document_id) == Some(app.active))
                .or_else(|| {
                    controller
                        .tabs()
                        .iter()
                        .find(|tab| self.document_index(workspace, tab.document_id) == Some(app.active))
                })
                .map(|tab| tab.id)
        });
        if let Some(id) = desired {
            if self.loaded_tabs[self.pane() as usize] != Some(id) {
                self.select_tab(workspace, app, id);
            }
        }
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
        self.install_views(workspace);
        self.tab_hits.clear();
        self.tab_nav.clear();
        self.tab_lists.clear();
        self.tab_strips = [None, None];
        let vertical = self
            .controller
            .as_ref()
            .is_some_and(|controller| controller.vertical_tabs);
        if !self.open() {
            let (inset, content_width) = self.find_horizontal_geometry(width);
            let mut local = Vec::new();
            let caret = workspace.draw(app.active, renderer, content_width, height, &mut local)?;
            self.status_labels = Self::drawn_status_labels(workspace, app.active, &local, height);
            ops.extend(local.into_iter().map(|op| translate(op, inset, 0.0)));
            self.bounds = [Some(rect(inset, 0.0, (width - inset).max(0.0), height - 24.0)), None];
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
            let start = ops.len();
            let caret = workspace.draw(app.active, renderer, width, height, ops)?;
            self.status_labels = Self::drawn_status_labels(workspace, app.active, &ops[start..], height);
            return Ok(caret);
        };
        self.refresh_find_to_active(workspace, notify.clone());
        let find_height = if workspace.find.open {
            workspace.find.height()
        } else {
            0.0
        };
        let panel_height = workspace.bottom_panel_height;
        let compare_height = 0.0;
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
                    rect(bounds.x, bounds.y, 176.0f32.min(bounds.width * 0.4), bounds.height)
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
        self.draw_tab_strip(workspace, pane, rect(0.0, 0.0, width, TAB_HEIGHT), false, ops);
        let paths = [
            workspace.path(first).map(std::path::Path::to_path_buf),
            workspace.path(second).map(std::path::Path::to_path_buf),
        ];
        for side in 0..2 {
            let Some(bounds) = self.bounds[side] else {
                continue;
            };
            let index = if side == 0 { first } else { second };
            let notice_band = if workspace.binary_warning_pending(index) {
                bareline_app::encoding::BINARY_NOTICE_HEIGHT
            } else {
                0.0
            };
            // Each pane reserves only the banner its own view draws (UI-02).
            let banner_band = if side == 0 {
                workspace.banner_band(index)
            } else {
                self.secondary_banner_band
            };
            let file_bytes = workspace.file_bytes(index);
            // A failed open shows its error panel in either pane (FIO-01).
            let failed = workspace
                .failed_open(index)
                .map(|(path, error)| (path.to_path_buf(), error.to_owned()));
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
                    editor.viewport_mut().error = Some(error);
                }
                self.applied_spacers[side] = Some(spacers);
            }
            // EditorSurface already reserves TAB_HEIGHT for this pane's header;
            // an external-change banner (UI-02) and a pending binary notice
            // (UI-01) each need a band under it.
            editor.viewport_mut().top_inset = banner_band + notice_band;
            editor.viewport_mut().bottom_inset = 0.0;
            editor.viewport_mut().file_bytes = file_bytes;
            editor.viewport_mut().not_loaded = failed.is_some();
            let paged = editor.paged();
            editor.set_external_scrollbar(paged);
            let mut local = Vec::new();
            let local_height = bounds.height + 24.0;
            self.styling[side].prepare_view(editor, paths[side].as_deref(), notify.clone());
            let syntax = self.styling[side].syntax_view(editor);
            let mut caret = if bareline_app::workspace::paint_paged_pending(
                editor,
                bounds.width,
                local_height,
                workspace.theme,
                &mut local,
            ) {
                None
            } else if let Some((path, error)) = &failed {
                bareline_app::workspace::paint_failed_open(
                    path,
                    error,
                    bounds.width,
                    local_height,
                    workspace.theme,
                    &mut local,
                );
                None
            } else {
                editor
                    .viewport_mut()
                    .draw_styled(renderer, bounds.width, local_height, &mut local, syntax)?
            };
            if let WorkspaceEditor::Paged(paged) = &mut *editor {
                if let Err(error) = paged.refine_horizontal_viewport(renderer, bounds.width) {
                    paged.error = Some(error);
                }
            }
            if matches!(&*editor, WorkspaceEditor::Paged(paged) if !paged.paged_frame_state().ready) {
                local.clear();
                bareline_app::workspace::paint_paged_pending(
                    editor,
                    bounds.width,
                    local_height,
                    workspace.theme,
                    &mut local,
                );
                caret = None;
            }
            if matches!(&*editor, WorkspaceEditor::Paged(paged) if !paged.caret_in_viewport()) {
                if let Some(rect) = caret.take() {
                    local.retain(|op| !matches!(op, DrawOp::Fill(bounds, _) if *bounds == rect));
                }
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
                // The pane fitted its labels to its own width; the status row
                // and footer refit the full ones instead (UI-07).
                if status.len() == editor.viewport().status_labels.len() {
                    status.clone_from(&editor.viewport().status_labels);
                }
                active_caret = caret.map(|r| rect(r.x + bounds.x, r.y + bounds.y, r.width, r.height));
            } else if let Some(caret) = caret {
                local.retain(|op| !matches!(op, DrawOp::Fill(r, _) if *r == caret));
            }
            ops.push(DrawOp::PushClip(bounds));
            ops.extend(local.into_iter().map(|op| translate(op, bounds.x, bounds.y)));
            ops.push(DrawOp::Fill(rect(bounds.x, bounds.y, bounds.width, TAB_HEIGHT), CHROME));
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
        self.status_labels.clone_from(&status);
        for (index, label) in status.into_iter().take(6).enumerate() {
            text(
                ops,
                12.0 + width * index as f32 / 6.0,
                height - 20.0,
                // Each group ends before the next one starts (UI-07).
                bareline_editor_surface::ellipsize_status(&label, width / 6.0 - 16.0),
                13.0,
                MUTED,
            );
        }
        if let Some(caret) = workspace
            .find
            .draw_with_theme_in(renderer, width, height, workspace.theme, ops)?
        {
            active_caret = Some(caret);
        }
        self.draw_mru(workspace, width, height, ops);
        Ok(active_caret)
    }
}

fn translate(op: DrawOp, x: f32, y: f32) -> DrawOp {
    let r = |r: Rect| rect(r.x + x, r.y + y, r.width, r.height);
    let p = |p: Point| Point { x: p.x + x, y: p.y + y };
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
        DrawOp::Layout { origin, layout, color } => DrawOp::Layout {
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
        DrawOp::Line { from, to, color, width } => DrawOp::Line {
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
        editor => editor.viewport_mut().scroll(delta, height),
    }
}
fn workspace_view_state(editor: &WorkspaceEditor) -> ViewState {
    let mut state = view_state(editor.viewport());
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
            let anchor = usize::try_from(state.anchor).map_err(|_| "Saved anchor exceeds this platform's range")?;
            let caret = usize::try_from(state.caret).map_err(|_| "Saved caret exceeds this platform's range")?;
            if anchor > editor.snapshot().len() || caret > editor.snapshot().len() {
                return Err("Saved selection is outside the restored document.".into());
            }
            if let Some(byte) = state.scroll_byte {
                let byte = usize::try_from(byte).map_err(|_| "Saved viewport exceeds this platform's range")?;
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
        restore_view(editor.viewport_mut(), state);
        return Ok(true);
    };
    if !editor.viewport_ready() {
        return Err("The saved viewport could not be loaded.".into());
    }
    if state.scroll_byte.is_some() {
        let token = if let Some(token) = *selection_token {
            token
        } else {
            let anchor = usize::try_from(state.anchor).map_err(|_| "Saved anchor exceeds this platform's range")?;
            let caret = usize::try_from(state.caret).map_err(|_| "Saved caret exceeds this platform's range")?;
            let token = editor.restore_global_selection(
                bareline_document::TextOffset(anchor),
                bareline_document::TextOffset(caret),
                true,
            )?;
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
        editor.viewport_mut().set_logical_scroll(0, 0.0, state.scroll_x as f64);
        editor.viewport_mut().scroll_y = f64::from_bits(state.scroll_y_bits);
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
        while !editor.snapshot().is_boundary(bareline_document::TextOffset(offset)) {
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
/// "All tabs" buttons, one per pane, above the previous/next ids of both panes.
const ACCESS_TAB_LIST_BASE: u64 = ACCESS_NAV_BASE + 8;
/// One tab list per drawn pane strip, so each pane's tabs form their own set.
const ACCESS_STRIP_BASE: u64 = 0x2800_0000_0000_0000;
/// Settings and Extensions page tabs and their close buttons, two ids each,
/// above the per-pane strip lists.
const ACCESS_PAGE_TAB_BASE: u64 = ACCESS_STRIP_BASE + 0x100;
fn access_tab_id(tab: u64) -> Option<u64> {
    tab.checked_mul(2)
        .and_then(|id| id.checked_add(ACCESS_TAB_BASE))
        .filter(|id| *id < ACCESS_NAV_BASE)
}
impl Shell {
    pub(super) fn views_accessibility_nodes(&self) -> Vec<bareline_platform::accessibility::AccessibilityNode> {
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
        // Every tab of a drawn strip is exposed, including tabs scrolled out of
        // it, with its position in that pane's set (A11Y-07). Only drawn tabs
        // have bounds and a close button.
        for (pane, strip) in self.views.tab_strips.iter().enumerate() {
            let Some(strip) = strip else {
                continue;
            };
            let pane = pane as u32;
            let tabs: Vec<_> = controller.pane_tabs(pane).collect();
            // The Settings and Extensions tabs drawn at the end of this strip
            // belong to the same set as its documents (UI-08).
            let pages: Vec<_> = self
                .views
                .tab_hits
                .iter()
                .filter(|hit| hit.pane == pane)
                .filter_map(|hit| Some((PageTab::from_tab_id(hit.id)?, *hit)))
                .collect();
            if tabs.is_empty() && pages.is_empty() {
                continue;
            }
            // While a page is shown no document tab reads as selected, as drawn.
            let page_open = pages.iter().any(|(page, _)| self.views.open_pages.contains(page));
            let list = ACCESS_STRIP_BASE + u64::from(pane);
            nodes.push(AccessibilityNode {
                id: list,
                parent: 1,
                role: AccessibilityRole::TabList,
                name: format!("Pane {} tabs", pane + 1),
                value: None,
                bounds: bounds(*strip),
                disabled: false,
                selected: false,
                expanded: None,
                focusable: false,
                invokable: false,
                position_in_set: None,
                size_of_set: None,
            });
            let documents = tabs.len();
            let size = documents + pages.len();
            for (position, tab) in tabs.into_iter().enumerate() {
                let Some(id) = access_tab_id(tab.id) else {
                    continue;
                };
                let Some(index) = self.views.tab_index(workspace, tab.id) else {
                    continue;
                };
                let hit = self
                    .views
                    .tab_hits
                    .iter()
                    .find(|hit| hit.id == tab.id && hit.pane == pane);
                let title = titles.get(index).cloned().unwrap_or_default();
                let name = format!(
                    "{}{}, pane {}{}",
                    title,
                    if tab.pinned { ", pinned" } else { "" },
                    pane + 1,
                    if workspace.editors[index].dirty() {
                        ", modified"
                    } else {
                        ""
                    }
                );
                nodes.push(AccessibilityNode {
                    id,
                    parent: list,
                    role: AccessibilityRole::Tab,
                    name,
                    value: controller.tab_colors.get(&tab.id).map(|color| format!("#{color:06x}")),
                    bounds: hit.map_or([0.0; 4], |hit| bounds(hit.bounds)),
                    disabled: self.views.busy(workspace),
                    selected: !page_open && controller.active_tab(pane) == Some(tab.id),
                    expanded: None,
                    focusable: true,
                    invokable: true,
                    position_in_set: Some(position + 1),
                    size_of_set: Some(size),
                });
                if let Some(hit) = hit {
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
                        position_in_set: None,
                        size_of_set: None,
                    });
                }
            }
            for (slot, (page, hit)) in pages.into_iter().enumerate() {
                let id = page.access_id();
                let name = page.accessible_name();
                nodes.push(AccessibilityNode {
                    id,
                    parent: list,
                    role: AccessibilityRole::Tab,
                    name: name.into(),
                    value: None,
                    bounds: bounds(hit.bounds),
                    disabled: false,
                    selected: self.views.open_pages.contains(&page),
                    expanded: None,
                    focusable: true,
                    invokable: true,
                    position_in_set: Some(documents + slot + 1),
                    size_of_set: Some(size),
                });
                nodes.push(AccessibilityNode {
                    id: id + 1,
                    parent: id,
                    role: AccessibilityRole::Button,
                    name: format!("Close {name}"),
                    value: None,
                    bounds: bounds(hit.close),
                    disabled: false,
                    selected: false,
                    expanded: None,
                    focusable: true,
                    invokable: true,
                    position_in_set: None,
                    size_of_set: None,
                });
            }
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
                position_in_set: None,
                size_of_set: None,
            });
        }
        for (pane, rect) in &self.views.tab_lists {
            let id = ACCESS_TAB_LIST_BASE + *pane as u64;
            if nodes.iter().any(|node| node.id == id) {
                continue;
            }
            nodes.push(AccessibilityNode {
                id,
                parent: 1,
                role: AccessibilityRole::Button,
                name: format!("All tabs in pane {}", pane + 1),
                value: None,
                bounds: bounds(*rect),
                disabled: false,
                selected: false,
                expanded: Some(self.views.mru_popup.as_ref().is_some_and(|popup| popup.list)),
                focusable: true,
                invokable: true,
                position_in_set: None,
                size_of_set: None,
            });
        }
        if let Some(popup) = &self.views.mru_popup {
            nodes.push(AccessibilityNode {
                id: ACCESS_MRU_BASE,
                parent: 1,
                role: AccessibilityRole::List,
                name: if popup.list { "All tabs" } else { "Recent documents" }.into(),
                value: None,
                bounds: bounds(popup.bounds),
                disabled: false,
                selected: false,
                expanded: Some(true),
                focusable: false,
                invokable: false,
                position_in_set: None,
                size_of_set: None,
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
                let Some(id) = access_tab_id(*tab).and_then(|id| id.checked_add(ACCESS_MRU_BASE)) else {
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
                    // Only the rows in view are nodes; their set is the whole list (UI-08).
                    position_in_set: Some(start + row + 1),
                    size_of_set: Some(popup.ids.len()),
                });
            }
        }
        nodes
    }
    pub(super) fn views_accessibility_focus(&self) -> Option<u64> {
        if let Some(popup) = &self.views.mru_popup {
            return access_tab_id(*popup.ids.get(popup.selected)?).and_then(|id| id.checked_add(ACCESS_MRU_BASE));
        }
        self.views
            .accessibility_focus
            .filter(|id| self.views_accessibility_nodes().iter().any(|node| node.id == *id))
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
            if let Some(position) = popup
                .ids
                .iter()
                .position(|tab| access_tab_id(*tab).and_then(|id| id.checked_add(ACCESS_MRU_BASE)) == Some(id))
            {
                popup.selected = position;
                if invoke {
                    let tab = popup.ids[position];
                    self.views.mru_popup = None;
                    if let Some(workspace) = &mut self.workspace
                        && self.views.select_tab(workspace, &mut self.app, tab)
                    {
                        self.views.leave_pages(&mut self.settings, &mut self.extensions);
                    }
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                return true;
            }
        }
        if let Some((pane, _)) = self
            .views
            .tab_lists
            .iter()
            .find(|(pane, _)| ACCESS_TAB_LIST_BASE + *pane as u64 == id)
            .copied()
        {
            self.views.accessibility_focus = if invoke { None } else { Some(id) };
            if invoke {
                self.views.open_tab_list(pane);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            return true;
        }
        if let Some((pane, forward, _)) = self
            .views
            .tab_nav
            .iter()
            .find(|(pane, forward, _)| ACCESS_NAV_BASE + *pane as u64 * 2 + u64::from(*forward) == id)
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
        if self.views_page_tab_accessibility(el, action) {
            return true;
        }
        // A scrolled-off tab has no hit but stays selectable (A11Y-07).
        let Some((tab, close)) = self
            .views
            .tab_hits
            .iter()
            .find(|hit| access_tab_id(hit.id).is_some_and(|base| id == base || id == base + 1))
            .map(|hit| (hit.id, access_tab_id(hit.id).is_some_and(|base| id == base + 1)))
            .or_else(|| {
                self.views
                    .controller
                    .as_ref()?
                    .tabs()
                    .iter()
                    .find(|tab| access_tab_id(tab.id) == Some(id))
                    .map(|tab| (tab.id, false))
            })
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
        if invoke && close {
            self.views.close_tab(workspace, &mut self.app, tab);
        } else if self.views.select_tab(workspace, &mut self.app, tab) && invoke {
            // Invoking a document tab leaves Settings/Extensions, as a click
            // does; focusing it alone does not (UI-09).
            self.views.leave_pages(&mut self.settings, &mut self.extensions);
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
    /// Focus or invoke a Settings/Extensions page tab or its close button, as a
    /// click on the strip does (UI-08). Also reachable while that page is shown.
    pub(super) fn views_page_tab_accessibility(
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
        let Some(page) = self
            .views
            .page_tabs()
            .into_iter()
            .find(|page| id == page.access_id() || id == page.access_id() + 1)
        else {
            return false;
        };
        self.views.accessibility_focus = if invoke { None } else { Some(id) };
        if invoke && id == page.access_id() + 1 {
            self.views.close_page(page, &mut self.settings, &mut self.extensions);
        } else if invoke && !self.views.open_pages.contains(&page) {
            // A parked page tab shows its page again through the page's command.
            self.dispatch(el, Action::Contributed(CommandId(page.command())));
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
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
                    if self.views.select_tab(workspace, &mut self.app, id) {
                        self.views.leave_pages(&mut self.settings, &mut self.extensions);
                    }
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
                        .filter(|id| controller.tab(*id).is_some_and(|tab| seen.insert(tab.document_id)))
                        .collect::<Vec<_>>();
                    let selected = usize::from(ids.len() > 1);
                    self.views.mru_popup = Some(MruPopup {
                        ids,
                        selected,
                        bounds: Rect::default(),
                        list: false,
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
    /// Select the document tab under the pointer so the tab right-click menu
    /// acts on it. Returns whether the pointer is over a document tab.
    pub(super) fn views_select_tab_under_pointer(&mut self) -> bool {
        let origin = self.editor_bounds();
        let point = Point {
            x: self.pointer.x - origin.x,
            y: self.pointer.y - origin.y,
        };
        let Some(hit) = self
            .views
            .tab_hits
            .iter()
            .find(|hit| hit.bounds.contains(point) && PageTab::from_tab_id(hit.id).is_none())
            .copied()
        else {
            return false;
        };
        if let Some(workspace) = &mut self.workspace
            && self.views.select_tab(workspace, &mut self.app, hit.id)
        {
            // Activating a document leaves any page shown over it (UI-09).
            self.views.leave_pages(&mut self.settings, &mut self.extensions);
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
        let mut show_page = None;
        let Some(workspace) = &mut self.workspace else {
            return false;
        };
        if self.views.mru_popup.is_some() {
            let mut accept = false;
            let mut cancel = false;
            let popup = self.views.mru_popup.as_mut().unwrap();
            match event {
                WindowEvent::ModifiersChanged(modifiers) if !popup.list && !modifiers.state().control_key() => {
                    accept = true
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    let down = match delta {
                        MouseScrollDelta::LineDelta(_, y) => *y < 0.0,
                        MouseScrollDelta::PixelDelta(point) => point.y < 0.0,
                    };
                    popup.selected = if down {
                        (popup.selected + 1).min(popup.ids.len().saturating_sub(1))
                    } else {
                        popup.selected.saturating_sub(1)
                    };
                }
                WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
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
                        popup.selected = (start + ((point.y - popup.bounds.y) / TAB_HEIGHT) as usize)
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
                if accept
                    && let Some(id) = popup.ids.get(popup.selected)
                    && self.views.select_tab(workspace, &mut self.app, *id)
                {
                    self.views.leave_pages(&mut self.settings, &mut self.extensions);
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
                    } else if let Some((pane, _)) = self
                        .views
                        .tab_lists
                        .iter()
                        .find(|(_, bounds)| bounds.contains(point))
                        .copied()
                    {
                        self.views.open_tab_list(pane);
                        handled = true;
                    } else if let Some(hit) = self
                        .views
                        .tab_hits
                        .iter()
                        .find(|hit| hit.bounds.contains(point))
                        .copied()
                    {
                        if let Some(page) = PageTab::from_tab_id(hit.id) {
                            // Page tab: × closes the page (parity with Ctrl+W and
                            // the header ×); the body shows the page again.
                            if hit.close.contains(point) {
                                self.views.close_page(page, &mut self.settings, &mut self.extensions);
                            } else if !self.views.open_pages.contains(&page) {
                                show_page = Some(page);
                            }
                        } else if hit.close.contains(point) {
                            self.views.close_tab(workspace, &mut self.app, hit.id);
                        } else {
                            if self.views.select_tab(workspace, &mut self.app, hit.id) {
                                self.views.leave_pages(&mut self.settings, &mut self.extensions);
                            }
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
                    let scale = self.window.as_ref().map_or(1.0, |window| window.scale_factor());
                    let p = position.to_logical::<f32>(scale);
                    self.pointer = Point { x: p.x, y: p.y };
                    let point = Point {
                        x: p.x - origin.x,
                        y: p.y - origin.y,
                    };
                    let drag = self.views.tab_drag.as_mut().unwrap();
                    drag.moved |= (point.x - drag.start.x).abs() + (point.y - drag.start.y).abs() > 5.0;
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
                            .find(|hit| hit.bounds.contains(point) && PageTab::from_tab_id(hit.id).is_none())
                            .map(|hit| (hit.pane, Some(hit.id)))
                            .or_else(|| {
                                self.views
                                    .bounds
                                    .iter()
                                    .enumerate()
                                    .find(|(_, bounds)| bounds.is_some_and(|bounds| bounds.contains(point)))
                                    .map(|(pane, _)| (pane as u32, None))
                            });
                        if let Some((pane, before)) = target {
                            self.views.save_current(workspace);
                            if let Some(controller) = &mut self.views.controller
                                && let Err(error) = controller.move_to_pane(drag.id, pane, before)
                            {
                                workspace.message = Some(format!("Tab cannot be moved: {error}."));
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
        if let Some(page) = show_page {
            // A parked page tab shows its page again through the page's command.
            self.dispatch(el, Action::Contributed(CommandId(page.command())));
            handled = true;
        }
        if handled && let Some(window) = &self.window {
            window.request_redraw();
        }
        handled
    }
    /// Start Close All/Others/Left/Right on the active tab strip (WSP-01). The
    /// tabs close one at a time through the normal close path, so each unsaved
    /// document still gets its own Save / Don't Save / Cancel prompt.
    fn tab_close_start(&mut self, id: &str) {
        if self.pending_close.is_some() || self.views.close_current.is_some() || !self.views.close_queue.is_empty() {
            if let Some(workspace) = &mut self.workspace {
                workspace.message = Some("Wait for the current close to finish.".into());
            }
            return;
        }
        let Some(workspace) = &mut self.workspace else {
            return;
        };
        self.views.sync_documents(workspace);
        self.views.save_current(workspace);
        let Some(controller) = &self.views.controller else {
            return;
        };
        let pane = controller.active_pane();
        let strip: Vec<(u64, bool)> = controller.pane_tabs(pane).map(|tab| (tab.id, tab.pinned)).collect();
        self.views.close_queue = close_targets(&strip, controller.active_tab(pane), id).into();
        self.tab_close_advance();
    }
    /// Close every document tab of both strips before File > Load Session
    /// (BIZ-07), one at a time through the normal close path, so each unsaved
    /// document still gets its own prompt. False while another close runs.
    pub(super) fn tab_close_everything(&mut self) -> bool {
        if self.tab_close_running() {
            return false;
        }
        let Some(workspace) = &mut self.workspace else {
            return true;
        };
        self.views.sync_documents(workspace);
        self.views.save_current(workspace);
        let Some(controller) = &self.views.controller else {
            return true;
        };
        let tabs: Vec<u64> = (0..=1u32)
            .flat_map(|pane| controller.pane_tabs(pane).map(|tab| tab.id))
            .collect();
        self.views.close_queue = tabs.into();
        self.tab_close_advance();
        true
    }
    /// A close, or a batch of Close All/Others/Left/Right, is still running.
    pub(super) fn tab_close_running(&self) -> bool {
        self.pending_close.is_some() || self.views.close_current.is_some() || !self.views.close_queue.is_empty()
    }
    /// Close the next queued tab once the previous close has settled. A tab
    /// still open after its close settled means the person chose Cancel or the
    /// close was refused, so the rest of the batch is dropped. A modal dialog
    /// or an exit in progress refuses the close, as it does for File > Close.
    pub(super) fn tab_close_advance(&mut self) {
        loop {
            if self.pending_close.is_some() || (self.views.close_queue.is_empty() && self.views.close_current.is_none())
            {
                return;
            }
            let Some(workspace) = &mut self.workspace else {
                self.views.close_queue.clear();
                self.views.close_current = None;
                return;
            };
            self.views.sync_documents(workspace);
            if let Some(current) = self.views.close_current.take()
                && self.views.has_tab(current)
            {
                self.views.close_queue.clear();
                return;
            }
            let Some(tab) = self.views.close_queue.pop_front() else {
                return;
            };
            if !self.views.has_tab(tab) {
                continue;
            }
            self.views.close_current = Some(tab);
            self.views.select_tab(workspace, &mut self.app, tab);
            self.views.close_tab(workspace, &mut self.app, tab);
            if self.views.pending_close.take().is_some() && self.modal.is_none() && !self.session.closing() {
                self.queue_active_close();
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }
    /// Window-menu activation of document `index`: select its tab and leave any
    /// Settings or Extensions page shown over the editor (UI-09).
    pub(super) fn select_window_document(&mut self, index: usize) {
        let Some(workspace) = &mut self.workspace else {
            return;
        };
        self.views.sync_documents(workspace);
        let tab_id = if let Some(controller) = self.views.controller.as_ref() {
            let ids: Vec<u64> = controller
                .pane_tabs(controller.active_pane())
                .map(|tab| tab.id)
                .collect();
            ids.into_iter()
                .find(|tab| self.views.tab_index(workspace, *tab) == Some(index))
        } else {
            None
        };
        let activated = match tab_id {
            Some(tab) => self.views.select_tab(workspace, &mut self.app, tab),
            None if index < workspace.editors.len() => {
                self.app.active = index;
                true
            }
            None => return,
        };
        if activated {
            self.views.leave_pages(&mut self.settings, &mut self.extensions);
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
    pub(super) fn views_dispatch(&mut self, _el: &ActiveEventLoop, id: &str) -> bool {
        if CLOSE_MULTIPLE_IDS.contains(&id) {
            self.tab_close_start(id);
            return true;
        }
        if let Some(index) = id
            .strip_prefix("window.select.")
            .and_then(|rest| rest.parse::<usize>().ok())
        {
            self.select_window_document(index);
            return true;
        }
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
            "view.split_vertical" => self.views.split(workspace, self.app.active, Orientation::Vertical),
            "view.clone_other" => self.views.clone_active(workspace, &mut self.app),
            "view.move_other" => self.views.move_active(workspace, &mut self.app),
            "view.split_horizontal" => self.views.split(workspace, self.app.active, Orientation::Horizontal),
            "view.close_split" => self.views.close_split(workspace),
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
        if matches!(action, Action::Save | Action::SaveAs | Action::Close | Action::Quit) && self.views.busy(workspace)
        {
            workspace.message = Some("Wait for pending split-view edits before saving or closing.".into());
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
                let limit = self
                    .platform
                    .as_ref()
                    .map_or(bareline_platform::clipboard::DEFAULT_CLIPBOARD_MAX_BYTES, |platform| {
                        platform.clipboard_max_bytes()
                    });
                let copied = editor.and_then(|e| e.selected_text(limit).ok()).is_some_and(|value| {
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
                    if let (Some(first), Some(second), Some(controller)) =
                        (self.views.bounds[0], self.views.bounds[1], &mut self.views.controller)
                    {
                        controller.ratio = if controller.orientation == Orientation::Vertical {
                            ((p.x - first.x) / (second.x + second.width - first.x)).clamp(0.1, 0.9) as f64
                        } else {
                            ((p.y - first.y) / (second.y + second.height - first.y)).clamp(0.1, 0.9) as f64
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
                    let local = Point {
                        x: pointer.x - bounds.x,
                        y: pointer.y - bounds.y,
                    };
                    let extend = self.modifiers.shift_key();
                    self.views
                        .press_pane(workspace, pane, local, self.renderer.as_ref(), extend);
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
                            MouseScrollDelta::PixelDelta(point) => (point.y / window.scale_factor() / 48.0) as f32,
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
                        MouseScrollDelta::LineDelta(x, _) if horizontal && !self.modifiers.shift_key() => {
                            -*x as f64 * 72.0
                        }
                        MouseScrollDelta::PixelDelta(p) if horizontal && !self.modifiers.shift_key() => {
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
                                if let WorkspaceEditor::Paged(editor) = &mut workspace.editors[index] {
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
                    self.views.secondary.as_mut().map(|editor| editor.viewport_mut())
                } else {
                    self.views
                        .primary_index(workspace)
                        .and_then(|i| workspace.editors.get_mut(i))
                        .map(|editor| editor.viewport_mut())
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
                    self.views.input(workspace, pane, Input::Insert(value.clone()));
                }
                handled = true;
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                // Tab and Shift+Tab must use the same effective shortcut and
                // focus routing as the primary editor (including user bindings).
                if event.logical_key == Key::Named(NamedKey::Tab) {
                    return false;
                }
                let extend = self.modifiers.shift_key();
                if matches!(event.logical_key, Key::Named(NamedKey::PageDown | NamedKey::PageUp))
                    && !self.modifiers.control_key()
                {
                    let forward = matches!(event.logical_key, Key::Named(NamedKey::PageDown));
                    let pane = self.views.pane();
                    let height = self.views.bounds[pane as usize].map_or(400.0, |bounds| bounds.height);
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
                    Key::Named(NamedKey::ArrowLeft) if self.modifiers.control_key() && !self.modifiers.alt_key() => {
                        Some(Input::WordLeft(extend))
                    }
                    Key::Named(NamedKey::ArrowLeft) => Some(Input::Left(extend)),
                    Key::Named(NamedKey::ArrowRight) if self.modifiers.control_key() && !self.modifiers.alt_key() => {
                        Some(Input::WordRight(extend))
                    }
                    Key::Named(NamedKey::ArrowRight) => Some(Input::Right(extend)),
                    Key::Named(NamedKey::ArrowUp) => Some(Input::Up(extend)),
                    Key::Named(NamedKey::ArrowDown) => Some(Input::Down(extend)),
                    Key::Named(NamedKey::Home) if self.modifiers.control_key() && !self.modifiers.alt_key() => {
                        Some(Input::DocumentHome(extend))
                    }
                    Key::Named(NamedKey::Home) => Some(Input::Home(extend)),
                    Key::Named(NamedKey::End) if self.modifiers.control_key() && !self.modifiers.alt_key() => {
                        Some(Input::DocumentEnd(extend))
                    }
                    Key::Named(NamedKey::End) => Some(Input::End(extend)),
                    Key::Named(NamedKey::Backspace) => Some(Input::Backspace),
                    Key::Named(NamedKey::Delete) => Some(Input::Delete),
                    Key::Named(NamedKey::Enter) => self
                        .views
                        .active_workspace_editor(workspace, self.app.active)
                        .map(|editor| Input::Insert(editor.snapshot().insertion_eol().into())),
                    _ if !self.modifiers.control_key() || self.modifiers.alt_key() => event
                        .text
                        .as_ref()
                        .filter(|text| !text.is_empty() && !text.chars().any(|c| c.is_control()))
                        .map(|text| Input::Insert(text.to_string())),
                    _ => None,
                };
                if event.logical_key == Key::Named(NamedKey::Insert)
                    && !self.modifiers.shift_key()
                    && !self.modifiers.control_key()
                    && !self.modifiers.alt_key()
                {
                    self.views.toggle_overwrite(workspace, self.app.active);
                    handled = true;
                }
                // In overwrite mode the surface replaces the next character when
                // it dequeues the keystroke (UI-07).
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
