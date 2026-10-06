// SPDX-License-Identifier: MPL-2.0
//! Offscreen whole-window render cells for the visual baselines (QA-13).
//!
//! Each cell composes a frame the way `render_frame` does — shell chrome and
//! tab strip, the editor layer with an open Find bar and scroll bars, and the
//! status footer — and renders it with the production Direct2D/DirectWrite
//! renderer onto a WIC bitmap at 100/150/200% in light and dark. No window,
//! desktop capture or input is used. The toolbar (hidden by default), side
//! panels and overlays need a live event loop and are not part of a cell.
//!
//! Pixels depend on the installed system fonts, so the capture is ignored by
//! default. `tests/visual/capture.ps1` runs it on the reference runner and
//! `tests/visual/compare.py` compares the cells with the reviewed baselines
//! under a tolerance.
use super::Shell;
use bareline_platform_windows::WindowsRenderer;
use bareline_renderer::{DrawOp, FrameStatus, RenderBackend};

const WIDTH: f32 = 960.0;
const HEIGHT: f32 = 600.0;
const SCALES: [f32; 3] = [1.0, 1.5, 2.0];
const SOURCE: &str = "use std::path::Path;\n\n/// Parsed server settings.\n#[derive(Debug)]\nstruct Config {\n    host: String,\n    port: u16,\n}\n\nfn load(path: &Path) -> Option<Config> {\n    // TODO: report parse errors\n    let text = std::fs::read_to_string(path).ok()?;\n    let port = text.len() as u16 % 1024 + 8000;\n    Some(Config { host: \"localhost\".into(), port })\n}\n\nfn main() {\n    let config = load(Path::new(\"config.toml\"));\n    println!(\"{config:?} — café 文 🎉\");\n}\n";

/// A headless shell whose only tab is the Rust fixture, themed light or dark.
fn cell_shell(dark: bool) -> Shell {
    let mut workspace = bareline_app::workspace::Workspace::new(
        std::sync::Arc::new(|| {}),
        std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
    )
    .unwrap();
    let document = bareline_document::Document::from_utf8(
        SOURCE,
        bareline_document::Budget::new(1 << 20),
        bareline_document::Budget::new(1 << 20),
    )
    .unwrap();
    let index = workspace
        .add_snapshot_preview(&document.snapshot(), "main.rs".into())
        .unwrap();
    let mut shell = super::accessibility::tests::headless_shell();
    shell.settings.controller.system.high_contrast = false;
    shell.settings.apply_window_theme(Some(if dark {
        winit::window::Theme::Dark
    } else {
        winit::window::Theme::Light
    }));
    let ui = shell.settings.ui_theme();
    let editor_theme = shell.settings.editor_theme();
    workspace.theme = ui;
    for editor in &mut workspace.editors {
        editor.viewport_mut().theme = editor_theme;
        editor.viewport_mut().language_override = Some(bareline_syntax::Language::Rust);
    }
    workspace.find.show();
    workspace.find.field.insert("port");
    shell.app.tabs = workspace.titles();
    shell.app.active = index;
    shell.workspace = Some(workspace);
    shell
}

/// Background work (highlighting, the Find count) settles before a capture.
fn settle(shell: &mut Shell) {
    let workspace = shell.workspace.as_mut().unwrap();
    for _ in 0..500 {
        workspace.pump();
        if !workspace.io_busy()
            && !workspace.editors.iter().any(|editor| editor.busy())
            && workspace.find.status != "Searching…"
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("visual cell did not settle: {:?}", workspace.find.status);
}

/// The frame `render_frame` would compose for this shell, without the toolbar,
/// panels and overlays.
fn compose(shell: &mut Shell, renderer: &mut WindowsRenderer, scale: f32) -> Vec<DrawOp> {
    let toolbar = shell.toolbar.controller.height();
    let bounds = bareline_ui::rect(0.0, toolbar, WIDTH, HEIGHT - toolbar);
    let mut ops = bareline_ui::shell_with_theme(
        WIDTH,
        HEIGHT,
        &shell.app.tabs,
        shell.app.active,
        false,
        shell.settings.ui_theme(),
    );
    let start = ops.len();
    ops.push(DrawOp::PushClip(bounds));
    let workspace = shell.workspace.as_mut().unwrap();
    shell.views.sync(workspace, &mut shell.app);
    shell
        .views
        .draw(
            workspace,
            &mut shell.app,
            renderer,
            bounds.width,
            bounds.height,
            &mut ops,
            std::sync::Arc::new(|| {}),
        )
        .expect("editor layer layout");
    // Scroll bars are measured in window coordinates but painted in the layer.
    let scrollbars = ops.len();
    let workspace = shell.workspace.as_ref().unwrap();
    shell
        .scrolling
        .draw(workspace, &shell.views, shell.app.active, bounds, &mut ops);
    super::translate_operations(&mut ops[scrollbars..], -bounds.x, -bounds.y);
    super::translate_operations(&mut ops[start + 1..], bounds.x, bounds.y);
    ops.push(DrawOp::PopClip);
    let labels = shell.views.status_labels.clone();
    let size = winit::dpi::PhysicalSize::new((WIDTH * scale) as u32, (HEIGHT * scale) as u32);
    shell.draw_footer(size, scale, &labels, &mut ops);
    ops
}

/// A top-down 32-bit BMP of premultiplied BGRA pixels (opaque here).
fn bmp(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(54 + pixels.len());
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((54 + pixels.len()) as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(width as i32).to_le_bytes());
    out.extend_from_slice(&(-(height as i32)).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0; 24]);
    out.extend_from_slice(pixels);
    out
}

#[test]
#[ignore = "pixels depend on system fonts; run by tests/visual/capture.ps1 on the reference runner"]
fn capture_whole_window_cells() {
    let output = std::path::PathBuf::from(
        std::env::var_os("BARELINE_VISUAL_OUTPUT").expect("set BARELINE_VISUAL_OUTPUT to a fresh absolute directory"),
    );
    assert!(output.is_absolute(), "BARELINE_VISUAL_OUTPUT must be absolute");
    std::fs::create_dir_all(&output).unwrap();
    for dark in [false, true] {
        for scale in SCALES {
            let (width, height) = ((WIDTH * scale) as u32, (HEIGHT * scale) as u32);
            let mut renderer = WindowsRenderer::offscreen(width, height, scale).expect("offscreen renderer");
            let mut shell = cell_shell(dark);
            // The first frame lays out and starts background work; capture a settled one.
            compose(&mut shell, &mut renderer, scale);
            settle(&mut shell);
            let ops = compose(&mut shell, &mut renderer, scale);
            assert_eq!(renderer.render(&ops).expect("offscreen frame"), FrameStatus::Presented);
            let pixels = renderer.pixels_bgra().expect("offscreen pixels");
            let name = format!(
                "{}-{}pct.bmp",
                if dark { "dark" } else { "light" },
                (scale * 100.0) as u32
            );
            std::fs::write(output.join(name), bmp(width, height, &pixels)).unwrap();
        }
    }
}
