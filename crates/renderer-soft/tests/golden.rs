// SPDX-License-Identifier: MPL-2.0
//! Raster goldens: a fixed operation list using every `DrawOp`, and a text
//! fidelity scene (combining marks, bidi lines, missing emoji), rendered
//! offscreen with the bundled font only and compared with checked-in PNGs.
//!
//! A pixel matches when every channel is within [`TOLERANCE`]; at most
//! [`MAX_MISMATCHED`] of the pixels may differ by more (antialiasing can vary
//! slightly between CPU architectures). After an intended rendering change,
//! regenerate the images and review them before committing:
//!
//! ```text
//! BARELINE_UPDATE_GOLDENS=1 cargo test -p bareline-renderer-soft --test golden
//! ```
//!
//! (PowerShell: `$env:BARELINE_UPDATE_GOLDENS=1; cargo test ...`.) The images
//! are written to `crates/renderer-soft/tests/golden`.
use bareline_renderer::{Color, DrawOp, Image, Point, Rect, RenderBackend, TextBackend, TextStyle, squiggle};
use bareline_renderer_soft::{BUNDLED_FONT_FAMILY, FontSource, SoftRenderer};
use std::path::PathBuf;

const TOLERANCE: u8 = 16;
/// Fraction of pixels allowed beyond the tolerance.
const MAX_MISMATCHED: f64 = 0.001;

fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
    Rect { x, y, width, height }
}
/// A 16 x 16 straight-alpha gradient with a transparent corner.
fn gradient() -> Image {
    let mut pixels = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16u8 {
        for x in 0..16u8 {
            pixels.extend_from_slice(&[x * 16, y * 16, 200, if x + y < 6 { 0 } else { 255 }]);
        }
    }
    Image::rgba(16, 16, pixels).unwrap()
}
/// The scene: 320 x 200 DIPs.
fn scene(renderer: &mut SoftRenderer) -> Vec<DrawOp> {
    let code = "fn main() { let 文 = \"e\u{301}\"; }";
    let layout = renderer
        .shape_with_font_family(code, 16.0, 300.0, BUNDLED_FONT_FAMILY)
        .unwrap();
    let keyword = 0..2;
    let string = code.find('"').unwrap()..code.rfind('"').unwrap() + 1;
    renderer
        .set_styles(
            layout,
            &[
                TextStyle {
                    bytes: keyword,
                    color: Color(0xFF7B72),
                },
                TextStyle {
                    bytes: string,
                    color: Color(0xA5D6FF),
                },
            ],
        )
        .unwrap();
    let paragraph = renderer
        .shape_wrapped(
            "Wrapped text breaks at words within its width.",
            12.0,
            150.0,
            BUNDLED_FONT_FAMILY,
        )
        .unwrap();
    let mut ops = vec![
        DrawOp::Fill(rect(0.0, 0.0, 320.0, 200.0), Color(0x1F2328)),
        DrawOp::Fill(rect(0.0, 0.0, 320.0, 30.0), Color(0x181B1F)),
        DrawOp::Stroke(rect(8.5, 4.5, 120.0, 22.0), Color(0x343A42), 1.0),
        DrawOp::Text {
            origin: Point { x: 14.0, y: 8.0 },
            text: "Untitled tab".into(),
            size: 13.0,
            color: Color(0xE6E8EA),
        },
        DrawOp::Fill(rect(8.0, 27.0, 121.0, 2.0), Color(0x2ED3C4)),
        DrawOp::Layout {
            origin: Point { x: 10.0, y: 38.0 },
            layout,
            color: Color(0xE6E8EA),
        },
        DrawOp::FillRounded(rect(12.0, 70.0, 90.0, 40.0), Color(0x262B31), 8.0),
        DrawOp::StrokeRounded(rect(12.0, 70.0, 90.0, 40.0), Color(0x2ED3C4), 8.0, 1.5),
        DrawOp::PushClip(rect(110.0, 70.0, 80.0, 60.0)),
        DrawOp::PushClip(rect(120.0, 60.0, 100.0, 40.0)),
        DrawOp::FillRounded(rect(100.0, 60.0, 120.0, 80.0), Color(0x22524A), 20.0),
        DrawOp::PopClip,
        DrawOp::Line {
            from: Point { x: 110.0, y: 130.0 },
            to: Point { x: 190.0, y: 70.0 },
            color: Color(0xF0B429),
            width: 2.0,
        },
        DrawOp::PopClip,
        DrawOp::PushLayer {
            bounds: rect(200.0, 40.0, 110.0, 70.0),
            opacity: 0.6,
        },
        DrawOp::Fill(rect(190.0, 30.0, 140.0, 90.0), Color(0x9AA3AD)),
        DrawOp::Image {
            image: gradient(),
            destination: rect(210.0, 50.0, 48.0, 48.0),
            opacity: 1.0,
        },
        DrawOp::PopLayer,
        DrawOp::Image {
            image: gradient(),
            destination: rect(270.0, 120.0, 32.0, 32.0),
            opacity: 0.5,
        },
        DrawOp::Layout {
            origin: Point { x: 12.0, y: 130.0 },
            layout: paragraph,
            color: Color(0x9AA3AD),
        },
        DrawOp::Text {
            origin: Point { x: 200.0, y: 172.0 },
            text: "Ctrl+Shift+P".into(),
            size: 13.0,
            color: Color(0x9AA3AD),
        },
    ];
    squiggle(200.0, 280.0, 192.0, Color(0xFF5555), &mut ops);
    ops
}
/// Text fidelity, 320 x 200 DIPs: spacing combining marks over their base and
/// a precomposed sequence (LNX-UI-009), lines that start right to left but read
/// left to right (LNX-UI-010), and one box per emoji cluster the bundled face
/// lacks (LNX-UI-017).
fn text_scene(renderer: &mut SoftRenderer) -> Vec<DrawOp> {
    let lines = [
        "Z\u{336}\u{335} o\u{302}\u{323} x\u{323}\u{307} end",
        "مرحبا 123 ok",
        "שלום abc",
        "Mixed: abc مرحبا def",
        "👍🏽 🇯🇵 ❤\u{FE0F} end",
    ];
    let mut ops = vec![DrawOp::Fill(rect(0.0, 0.0, 320.0, 200.0), Color(0x1F2328))];
    for (index, line) in lines.iter().enumerate() {
        let layout = renderer
            .shape_with_font_family(line, 16.0, 300.0, BUNDLED_FONT_FAMILY)
            .unwrap();
        ops.push(DrawOp::Layout {
            origin: Point {
                x: 10.0,
                y: 10.0 + 36.0 * index as f32,
            },
            layout,
            color: Color(0xE6E8EA),
        });
    }
    ops
}

fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join(name)
}

fn check_golden(name: &str, scale: f32) {
    check_golden_of(name, scale, scene);
}
fn check_golden_of(name: &str, scale: f32, scene: fn(&mut SoftRenderer) -> Vec<DrawOp>) {
    let (width, height) = ((320.0 * scale) as u32, (200.0 * scale) as u32);
    let mut renderer = SoftRenderer::offscreen_with_fonts(width, height, scale, FontSource::BundledOnly).unwrap();
    let ops = scene(&mut renderer);
    renderer.render(&ops).unwrap();
    let path = golden_path(name);
    if std::env::var_os("BARELINE_UPDATE_GOLDENS").is_some() {
        renderer.write_png(&path).unwrap();
        return;
    }
    let expected = tiny_skia::Pixmap::load_png(&path)
        .unwrap_or_else(|error| panic!("{}: {error}; regenerate with BARELINE_UPDATE_GOLDENS=1", path.display()));
    let (actual_width, actual_height, actual) = renderer.frame_rgba().unwrap();
    assert_eq!((expected.width(), expected.height()), (actual_width, actual_height));
    let mut mismatched = 0usize;
    let mut worst = 0u8;
    for (a, b) in actual.as_chunks::<4>().0.iter().zip(expected.data().as_chunks::<4>().0) {
        let difference = a.iter().zip(b).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
        worst = worst.max(difference);
        if difference > TOLERANCE {
            mismatched += 1;
        }
    }
    let allowed = (f64::from(actual_width * actual_height) * MAX_MISMATCHED) as usize;
    if mismatched > allowed {
        let failed = std::env::temp_dir().join(format!("actual-{name}"));
        renderer.write_png(&failed).unwrap();
        panic!(
            "{name}: {mismatched} pixels differ by more than {TOLERANCE} (allowed {allowed}, worst {worst}); \
             actual frame written to {}",
            failed.display()
        );
    }
}

#[test]
fn every_draw_op_matches_the_golden_at_scale_1() {
    check_golden("scene-scale1.png", 1.0);
}
#[test]
fn every_draw_op_matches_the_golden_at_scale_1_5() {
    check_golden("scene-scale1_5.png", 1.5);
}
#[test]
fn combining_bidi_and_missing_emoji_text_matches_the_golden() {
    check_golden_of("text-scale1.png", 1.0, text_scene);
    check_golden_of("text-scale2.png", 2.0, text_scene);
}
#[test]
fn the_comparison_detects_a_changed_frame() {
    // Guards the golden check itself: one recoloured operation must fail it.
    let mut renderer = SoftRenderer::offscreen_with_fonts(320, 200, 1.0, FontSource::BundledOnly).unwrap();
    let mut ops = scene(&mut renderer);
    ops[6] = DrawOp::FillRounded(rect(12.0, 70.0, 90.0, 40.0), Color(0xFFFFFF), 8.0);
    renderer.render(&ops).unwrap();
    let Ok(expected) = tiny_skia::Pixmap::load_png(golden_path("scene-scale1.png")) else {
        return;
    };
    let differing = renderer
        .frame_rgba()
        .unwrap()
        .2
        .as_chunks::<4>()
        .0
        .iter()
        .zip(expected.data().as_chunks::<4>().0)
        .filter(|(a, b)| a.iter().zip(b.iter()).any(|(a, b)| a.abs_diff(*b) > TOLERANCE))
        .count();
    assert!(differing as f64 > 320.0 * 200.0 * MAX_MISMATCHED, "{differing}");
}
