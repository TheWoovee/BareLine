// SPDX-License-Identifier: MPL-2.0
use bareline_renderer::{Color, DrawOp, RenderBackend};
use bareline_renderer_recording::RecordingBackend;
use bareline_ui::{
    ViewId, controls::*, overlays::*, rect, text_field::TextField, theme::UiTheme,
    variable_list::*, virtual_tree::*, widgets::*,
};

struct Rows;
impl ItemSource for Rows {
    fn len(&self) -> Option<usize> {
        Some(3)
    }
    fn discovered(&self) -> usize {
        3
    }
    fn label(&self, i: usize) -> &str {
        ["First", "Disabled", "Last"][i]
    }
    fn enabled(&self, i: usize) -> bool {
        i != 1
    }
}
impl TreeSource for Rows {
    fn child_count(&self, _: Option<NodeId>) -> Option<usize> {
        Some(3)
    }
    fn child(&self, _: Option<NodeId>, i: usize) -> Option<TreeItem<'_>> {
        (i < 3).then(|| TreeItem {
            id: NodeId(i as u64),
            label: ItemSource::label(self, i),
            expandable: false,
            enabled: i != 1,
        })
    }
}
impl VariableItemSource for Rows {
    fn len(&self) -> Option<usize> {
        Some(3)
    }
    fn discovered(&self) -> usize {
        3
    }
    fn index_at(&self, offset: f64) -> usize {
        if offset < 28.0 {
            0
        } else if offset < 70.0 {
            1
        } else {
            2
        }
    }
    fn offset_of(&self, i: usize) -> f64 {
        [0.0, 28.0, 70.0][i]
    }
    fn item_height(&self, i: usize) -> f32 {
        [28.0, 42.0, 28.0][i]
    }
    fn label(&self, i: usize) -> &str {
        ItemSource::label(self, i)
    }
}
fn list() -> List {
    List {
        bounds: rect(0.0, 36.0, 200.0, 84.0),
        state: ControlState {
            focused: true,
            ..Default::default()
        },
        metrics: Metrics::COMPACT,
        offset: 0.0,
        selected: Some(0),
    }
}
fn op_color(op: &DrawOp) -> Option<Color> {
    match op {
        DrawOp::Fill(_, c)
        | DrawOp::Stroke(_, c, _)
        | DrawOp::FillRounded(_, c, _)
        | DrawOp::StrokeRounded(_, c, _, _) => Some(*c),
        DrawOp::Text { color, .. } | DrawOp::Layout { color, .. } | DrawOp::Line { color, .. } => {
            Some(*color)
        }
        _ => None,
    }
}

#[test]
fn every_control_records_resolved_theme_at_required_scales() {
    use bareline_settings::{SystemAppearance, Theme as SettingsTheme, ThemeMode};
    let mut snapshot = String::new();
    for (name, dark, high_contrast) in [
        ("light", false, false),
        ("dark", true, false),
        ("highcontrast", true, true),
    ] {
        let tokens = SettingsTheme::resolve(
            ThemeMode::System,
            SystemAppearance {
                dark,
                high_contrast,
            },
            &Default::default(),
        )
        .unwrap();
        let theme =
            UiTheme::from_tokens(|key| tokens.color(key).map(|c| (c.rgb, c.alpha))).unwrap();
        assert!(
            tokens
                .color("text")
                .unwrap()
                .contrast(tokens.color("surface.elevated").unwrap())
                >= 4.5
        );
        let allowed = [
            theme.editor,
            theme.chrome,
            theme.elevated,
            theme.text,
            theme.muted,
            theme.border,
            theme.interactive,
            theme.focus,
            theme.selection,
            theme.caret,
        ];
        let mut backend = RecordingBackend::default();
        let mut cases: Vec<(&str, Vec<DrawOp>)> = Vec::new();
        macro_rules! record {
            ($name:expr, $paint:expr) => {{
                let mut ops = Vec::new();
                ($paint)(&mut ops);
                cases.push(($name, ops));
            }};
        }
        let bounds = rect(0.0, 0.0, 200.0, 28.0);
        let state = ControlState {
            focused: true,
            checked: true,
            ..Default::default()
        };
        for (name, toggle) in [("button", false), ("toggle", true)] {
            let button = Button {
                id: ViewId(1),
                label: "Control".into(),
                bounds,
                state,
                toggle,
            };
            record!(name, |ops| button.paint_with_theme(theme, ops));
        }
        record!("checkbox", |ops| Checkbox {
            id: ViewId(2),
            bounds,
            state
        }
        .paint("Enabled", theme.widgets(), ops));
        record!("radio", |ops| RadioGroup { list: list() }.paint(
            &Rows,
            theme.widgets(),
            ops
        ));
        record!("list", |ops| list().paint(&Rows, theme.widgets(), ops));
        let mut combo = Combo::new(list());
        combo.set_open(true);
        record!("combo", |ops| combo.paint(
            bounds,
            &Rows,
            theme.widgets(),
            ops
        ));
        let range = NumberRange::new(0.0, 100.0, 5.0, 50.0).unwrap();
        let mut slider = Slider::new(bounds, range);
        slider.state = state;
        record!("slider", |ops| slider.paint(theme.widgets(), ops));
        record!("stepper", |ops| Stepper {
            bounds,
            state,
            range
        }
        .paint(theme.widgets(), ops));
        let mut splitter = Splitter::new(rect(100.0, 0.0, 6.0, 100.0), true, range);
        splitter.state = state;
        record!("splitter", |ops| splitter.paint(theme.widgets(), ops));
        record!("scrollbar", |ops| Scrollbar {
            bounds: rect(0.0, 0.0, 16.0, 120.0),
            offset: 30.0,
            viewport: 100.0,
            total: Some(1000.0)
        }
        .paint_with_theme(theme, ops));
        let mut tree = Tree::new(rect(0.0, 0.0, 200.0, 84.0));
        tree.state = state;
        record!("tree", |ops| tree.paint(&Rows, theme.widgets(), ops));
        record!("variable-list", |ops| VariableList {
            bounds: rect(0.0, 0.0, 200.0, 98.0),
            offset: 0.0,
            selected: Some(0)
        }
        .paint(&Rows, theme.widgets(), ops));
        let panel = AnchoredPanel {
            popover: Popover::place(ViewId(1), bounds, 200.0, 60.0, rect(0.0, 0.0, 240.0, 160.0)),
            sticky: false,
        };
        record!("popover", |ops| panel.paint(theme.widgets(), ops));
        let mut tooltip = Tooltip::default();
        tooltip.hover(true, 0);
        record!("tooltip", |ops| tooltip.paint(
            500,
            bounds,
            "Help",
            theme.widgets(),
            ops
        ));
        record!("banner", |ops| Banner::new(
            ViewId(1),
            bounds,
            "Saved".into(),
            ViewId(2)
        )
        .paint(theme.widgets(), ops));
        record!("tabs-shell", |ops: &mut Vec<DrawOp>| ops.extend(
            bareline_ui::shell_with_theme(240.0, 160.0, &["File".into()], 0, false, theme)
        ));
        let mut field = TextField::default();
        field.insert("Selected");
        field.select_all();
        record!("text-field", |ops| {
            field
                .draw_with_theme(&mut backend, bounds, true, theme, ops)
                .unwrap();
        });
        for (control, ops) in &cases {
            assert!(!ops.is_empty(), "{name}/{control}");
            for color in ops.iter().filter_map(op_color) {
                assert!(
                    allowed.contains(&color),
                    "unresolved paint {name}/{control}: {color:?}"
                );
            }
            if [
                "button",
                "toggle",
                "checkbox",
                "radio",
                "list",
                "combo",
                "slider",
                "stepper",
                "splitter",
                "tree",
                "text-field",
            ]
            .contains(control)
            {
                assert!(
                    ops.iter().any(|op| op_color(op) == Some(theme.focus)),
                    "missing focus {name}/{control}"
                );
            }
            snapshot.push_str(&format!("{name}/{control}\n{ops:?}\n"));
            for scale in [1.0, 1.5, 2.0, 3.0] {
                backend
                    .resize((240.0 * scale) as u32, (160.0 * scale) as u32, scale)
                    .unwrap();
                backend.render(ops).unwrap();
                assert_eq!(backend.operations.len(), ops.len());
            }
        }
        field.release(&mut backend);
    }
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/controls.txt");
    if std::env::var_os("BARELINE_UPDATE_GOLDENS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &snapshot).unwrap();
    }
    assert_eq!(
        std::fs::read_to_string(path).unwrap().replace("\r\n", "\n"),
        snapshot
    );
}
