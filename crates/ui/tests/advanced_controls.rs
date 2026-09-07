// SPDX-License-Identifier: MPL-2.0
use bareline_renderer::{DrawOp, Point, balanced_clips};
use bareline_renderer_recording::RecordingBackend;
use bareline_ui::controls::{
    ControlAction, Key, Popover, ScrollAction, Scrollbar, ScrollbarInteraction, UiEvent,
};
use bareline_ui::focus::{DispatchResult, EventRouter, RouteTarget};
use bareline_ui::overlays::{AnchoredPanel, Banner, Tooltip};
use bareline_ui::text_field::{TextField, TextMode};
use bareline_ui::variable_list::{VariableItemSource, VariableList};
use bareline_ui::virtual_tree::{NodeId, Tree, TreeAction, TreeItem, TreeSource};
use bareline_ui::widgets::Theme;
use bareline_ui::{ViewId, rect};
use std::cell::Cell;

struct MillionTree {
    reads: Cell<usize>,
    loaded: Cell<bool>,
}
#[test]
fn focus_loss_cancels_capture_even_when_popup_handles_and_target_was_disabled() {
    let mut router = EventRouter {
        popup_capture: Some(ViewId(1)),
        pointer_capture: Some(ViewId(3)),
    };
    let targets = [1, 2, 3].map(|id| RouteTarget {
        id: ViewId(id),
        parent: None,
        bounds: rect(0.0, 0.0, 20.0, 20.0),
        enabled: id != 3,
    });
    let mut visited = Vec::new();
    let result = router.route(
        UiEvent::Focus(false),
        Some(ViewId(2)),
        &targets,
        |id, event| {
            assert!(matches!(event, UiEvent::Focus(false)));
            visited.push(id);
            DispatchResult {
                handled: true,
                invalidated: None,
            }
        },
    );
    assert_eq!(visited, [ViewId(1), ViewId(2), ViewId(3)]);
    assert!(result.handled);
    assert_eq!(router.pointer_capture, None);
}
#[test]
fn discovered_tree_children_refresh_expansion_and_prune_removed_descendants() {
    let source = MillionTree {
        reads: Cell::new(0),
        loaded: Cell::new(true),
    };
    let mut tree = Tree::new(rect(0.0, 0.0, 300.0, 56.0));
    tree.event(UiEvent::Focus(true), &source);
    tree.event(UiEvent::Key(Key::Home), &source);
    tree.expand_selected(&source);
    tree.update_child_count(NodeId(1), 1);
    assert_eq!(tree.rows(&source), Some(1_000_001));
    tree.event(UiEvent::Key(Key::Right), &source);
    tree.expand_selected(&source);
    assert_eq!(tree.rows(&source), Some(1_000_003));
    tree.update_child_count(NodeId(1), 0);
    assert_eq!(tree.rows(&source), Some(1_000_000));
    assert_eq!(
        tree.selected_semantics(ViewId(1), "Node", "tree.select", &source)
            .unwrap()
            .value
            .as_deref(),
        Some("node")
    );
    tree.update_child_count(NodeId(1), 10);
    assert_eq!(tree.rows(&source), Some(1_000_010));
}

#[test]
fn visible_semantics_preserve_disabled_rows_and_router_reports_consumption() {
    use bareline_ui::widgets::{ItemSource, List, Metrics};
    struct Rows;
    impl ItemSource for Rows {
        fn len(&self) -> Option<usize> {
            Some(1_000_000)
        }
        fn discovered(&self) -> usize {
            1_000_000
        }
        fn label(&self, _: usize) -> &str {
            "painted"
        }
        fn enabled(&self, i: usize) -> bool {
            i != 1
        }
    }
    let list = List {
        bounds: rect(0.0, 0.0, 200.0, 56.0),
        offset: 0.0,
        selected: Some(0),
        state: Default::default(),
        metrics: Metrics::COMPACT,
    };
    let nodes = bareline_ui::semantics::list(
        &list,
        &Rows,
        ViewId(1),
        |i| (ViewId(i as u64 + 2), format!("Localized {i}")),
        "list.select",
    );
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0].node.name, "Localized 0");
    assert!(nodes[1].node.disabled);
    assert!(nodes[1].node.actions.is_empty());
    let mut router = EventRouter::default();
    let result = router.route(
        UiEvent::Key(Key::Enter),
        Some(ViewId(1)),
        &[RouteTarget {
            id: ViewId(1),
            parent: None,
            bounds: list.bounds,
            enabled: true,
        }],
        |_, _| DispatchResult {
            handled: true,
            invalidated: None,
        },
    );
    assert!(result.handled);
    assert!(result.invalidated.is_empty());
}
#[test]
fn deferred_scrollbar_commits_on_release_and_unknown_extent_refines_without_jump() {
    let mut scrollbar = Scrollbar {
        bounds: rect(0.0, 0.0, 16.0, 500.0),
        offset: 1000.0,
        viewport: 100.0,
        total: None,
    };
    let mut interaction = ScrollbarInteraction::default();
    interaction.deferred = true;
    let initial = scrollbar.offset;
    let thumb = scrollbar.thumb();
    interaction.event(
        &mut scrollbar,
        UiEvent::PointerDown(Point {
            x: 8.0,
            y: thumb.y + 4.0,
        }),
        true,
        true,
        10.0,
    );
    assert!(matches!(
        interaction.event(
            &mut scrollbar,
            UiEvent::PointerMove(Point { x: 8.0, y: 100.0 }),
            true,
            true,
            10.0
        ),
        Some(ScrollAction::Preview(_))
    ));
    interaction.event(&mut scrollbar, UiEvent::Focus(false), true, false, 10.0);
    assert_eq!(scrollbar.offset, initial);
    let thumb = scrollbar.thumb();
    interaction.event(
        &mut scrollbar,
        UiEvent::PointerDown(Point {
            x: 8.0,
            y: thumb.y + 4.0,
        }),
        true,
        true,
        10.0,
    );
    assert!(matches!(
        interaction.event(
            &mut scrollbar,
            UiEvent::PointerUp(Point { x: 8.0, y: 100.0 }),
            true,
            true,
            10.0
        ),
        Some(ScrollAction::Commit(_))
    ));
    let mut settled = false;
    for _ in 0..100 {
        let before = scrollbar.thumb();
        settled = interaction.refine_total(&mut scrollbar, 1_000_000.0);
        let after = scrollbar.thumb();
        assert!((after.y - before.y).abs() <= before.height + 0.001);
        assert!((after.height - before.height).abs() <= before.height + 0.001);
        if settled {
            break;
        }
    }
    assert!(settled);
    assert_eq!(scrollbar.total, Some(1_000_000.0));
}
impl TreeSource for MillionTree {
    fn child_count(&self, parent: Option<NodeId>) -> Option<usize> {
        match parent {
            None => Some(1_000_000),
            Some(NodeId(1)) if self.loaded.get() => Some(1_000_000),
            Some(NodeId(1)) => None,
            Some(NodeId(1_000_001)) => Some(2),
            _ => Some(0),
        }
    }
    fn child(&self, parent: Option<NodeId>, index: usize) -> Option<TreeItem<'_>> {
        self.reads.set(self.reads.get() + 1);
        let id = match parent {
            None => index as u64 + 1,
            Some(NodeId(1)) => index as u64 + 1_000_001,
            Some(NodeId(1_000_001)) => index as u64 + 2_000_001,
            _ => return None,
        };
        Some(TreeItem {
            id: NodeId(id),
            label: "node",
            expandable: id == 1 || id == 1_000_001,
            enabled: true,
        })
    }
}
#[test]
fn lazy_tree_seeks_million_siblings_and_requests_only_expanded_children() {
    let source = MillionTree {
        reads: Cell::new(0),
        loaded: Cell::new(false),
    };
    let mut tree = Tree::new(rect(0.0, 0.0, 300.0, 56.0));
    tree.event(UiEvent::Focus(true), &source);
    assert_eq!(
        tree.event(UiEvent::Key(Key::Home), &source),
        Some(TreeAction::Selected(NodeId(1)))
    );
    assert_eq!(
        tree.event(UiEvent::Key(Key::Right), &source),
        Some(TreeAction::RequestChildren(Some(NodeId(1))))
    );
    assert_eq!(tree.rows(&source), Some(1_000_000));
    source.loaded.set(true);
    assert_eq!(
        tree.event(UiEvent::Key(Key::Right), &source),
        Some(TreeAction::Expanded(NodeId(1)))
    );
    assert_eq!(tree.rows(&source), Some(2_000_000));
    assert_eq!(
        tree.event(UiEvent::Key(Key::Right), &source),
        Some(TreeAction::Selected(NodeId(1_000_001)))
    );
    assert_eq!(
        tree.event(UiEvent::Key(Key::Right), &source),
        Some(TreeAction::Expanded(NodeId(1_000_001)))
    );
    assert_eq!(tree.rows(&source), Some(2_000_002));
    assert_eq!(
        tree.event(UiEvent::Key(Key::End), &source),
        Some(TreeAction::Selected(NodeId(1_000_000)))
    );
    source.reads.set(0);
    tree.offset = 500_000.0 * 28.0;
    let mut ops = Vec::new();
    tree.paint(&source, Theme::default(), &mut ops);
    assert_eq!(
        source.reads.get(),
        4,
        "viewport does not enumerate preceding siblings"
    );
    assert!(balanced_clips(&ops));
    tree.event(UiEvent::Key(Key::Home), &source);
    assert_eq!(
        tree.event(UiEvent::Key(Key::Left), &source),
        Some(TreeAction::Collapsed(NodeId(1)))
    );
    assert_eq!(
        tree.rows(&source),
        Some(1_000_000),
        "collapse removes descendant expansion counts"
    );
}

struct VariableSource {
    labels: Cell<usize>,
}
impl VariableItemSource for VariableSource {
    fn len(&self) -> Option<usize> {
        None
    }
    fn discovered(&self) -> usize {
        1_000_000
    }
    fn index_at(&self, offset: f64) -> usize {
        let pair = (offset.max(0.0) / 56.0).floor() as usize;
        pair * 2 + usize::from(offset - pair as f64 * 56.0 >= 16.0)
    }
    fn offset_of(&self, index: usize) -> f64 {
        (index / 2) as f64 * 56.0 + if index.is_multiple_of(2) { 0.0 } else { 16.0 }
    }
    fn item_height(&self, index: usize) -> f32 {
        if index.is_multiple_of(2) { 16.0 } else { 40.0 }
    }
    fn label(&self, _index: usize) -> &str {
        self.labels.set(self.labels.get() + 1);
        "row"
    }
}
#[test]
fn indexed_variable_rows_align_hit_testing_and_reveal_without_prefix_scan() {
    let source = VariableSource {
        labels: Cell::new(0),
    };
    let mut list = VariableList {
        bounds: rect(0.0, 0.0, 200.0, 56.0),
        offset: 500_000.0 * 28.0 + 8.0,
        selected: None,
    };
    assert_eq!(
        list.hit_test(&source, Point { x: 5.0, y: 1.0 }),
        Some(500_000)
    );
    assert_eq!(
        list.hit_test(&source, Point { x: 5.0, y: 9.0 }),
        Some(500_001)
    );
    let mut ops = Vec::new();
    list.paint(&source, Theme::default(), &mut ops);
    assert!(source.labels.get() <= 5);
    assert!(balanced_clips(&ops));
    assert!(list.reveal(&source, 999_999));
    assert_eq!(list.selected, Some(999_999));
    assert_eq!(list.offset + 56.0, source.offset_of(999_999) + 40.0);
}

#[test]
fn routing_capture_focus_hit_ancestors_and_release_are_ordered() {
    let bounds = rect(0.0, 0.0, 100.0, 100.0);
    let targets: Vec<_> = [(1, None), (2, Some(1)), (3, Some(1)), (4, None)]
        .into_iter()
        .map(|(id, parent)| RouteTarget {
            id: ViewId(id),
            parent: parent.map(ViewId),
            bounds,
            enabled: true,
        })
        .collect();
    let mut router = EventRouter {
        popup_capture: Some(ViewId(4)),
        pointer_capture: Some(ViewId(3)),
    };
    let mut seen = Vec::new();
    let regions = router.dispatch(
        UiEvent::PointerUp(Point { x: 500.0, y: 500.0 }),
        Some(ViewId(2)),
        &targets,
        |id, _| {
            seen.push(id);
            DispatchResult {
                handled: false,
                invalidated: Some(bounds),
            }
        },
    );
    assert_eq!(seen, [ViewId(4), ViewId(2), ViewId(3), ViewId(1)]);
    assert_eq!(regions.len(), 4);
    assert_eq!(router.pointer_capture, None);
    seen.clear();
    router.dispatch(
        UiEvent::Key(Key::Escape),
        Some(ViewId(2)),
        &targets,
        |id, _| {
            seen.push(id);
            DispatchResult {
                handled: true,
                invalidated: None,
            }
        },
    );
    assert_eq!(seen, [ViewId(4)]);
}

#[test]
fn password_masks_layout_clipboard_and_semantics_while_decimal_rejects_invalid_commit() {
    let mut field = TextField::default();
    field.set_mode(TextMode::Password);
    field.insert("e\u{301}🔒x");
    field.select_all();
    assert_eq!(field.selected(), "");
    assert_eq!(field.semantic_value(), "•••");
    let mut backend = RecordingBackend::default();
    let mut ops = Vec::new();
    field
        .draw(&mut backend, rect(0.0, 0.0, 150.0, 28.0), true, &mut ops)
        .unwrap();
    field
        .click(&backend, Point { x: 8.0, y: 14.0 }, false)
        .unwrap();
    field.delete(true);
    assert_eq!(
        field.value(),
        "🔒x",
        "masked hit maps back to one complete grapheme"
    );
    assert!(!field.set_mode(TextMode::Decimal));
    field.select_all();
    field.insert("");
    assert!(field.set_mode(TextMode::Decimal));
    assert!(field.insert("-12.5"));
    field.select_all();
    field.preedit("invalid".into(), None);
    assert!(!field.commit("invalid"));
    assert!(!field.composing());
    assert_eq!(field.value(), "-12.5");
    field.undo(false);
    assert_eq!(
        field.value(),
        "",
        "rejected numeric composition does not add history"
    );
    assert!(
        !ops.iter()
            .any(|op| matches!(op, DrawOp::Text { text, .. } if text.contains('🔒')))
    );
}

#[test]
fn sticky_panel_tooltip_deadline_and_banner_dismiss_are_explicit() {
    let bounds = rect(10.0, 10.0, 120.0, 28.0);
    let mut panel = AnchoredPanel {
        popover: Popover::place(
            ViewId(1),
            bounds,
            120.0,
            100.0,
            rect(0.0, 0.0, 200.0, 200.0),
        ),
        sticky: true,
    };
    assert_eq!(panel.event(UiEvent::Key(Key::Escape)), None);
    assert_eq!(panel.dismiss(), Some(ControlAction::Dismissed(ViewId(1))));
    assert_eq!(panel.dismiss(), None);
    let mut tooltip = Tooltip::default();
    tooltip.hover(true, 100);
    tooltip.hover(true, 200);
    assert_eq!(tooltip.deadline(), Some(600));
    assert!(!tooltip.visible(599));
    assert!(tooltip.visible(600));
    tooltip.hover(false, 601);
    assert!(!tooltip.visible(1000));
    let mut banner = Banner::new(
        ViewId(2),
        bounds,
        "Warning: changes not saved".into(),
        ViewId(3),
    );
    banner.event(UiEvent::Focus(true));
    assert_eq!(
        banner.event(UiEvent::Key(Key::Enter)),
        Some(ControlAction::Dismissed(ViewId(2)))
    );
    assert!(
        banner
            .semantics("Warning", "Dismiss warning", "warning.dismiss")
            .is_empty()
    );
}
