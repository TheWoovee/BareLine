// SPDX-License-Identifier: MPL-2.0
//! Offscreen frames: clips, layers, images, styled text, frame validation and
//! the shared shell, checked pixel by pixel.
use bareline_renderer::{
    Color, DrawOp, FrameStatus, Image, LayoutError, LayoutId, Point, Rect, RenderBackend, TextBackend, TextStyle,
};
use bareline_renderer_soft::{FontSource, SoftError, SoftRenderer};

fn renderer(width: u32, height: u32, scale: f32) -> SoftRenderer {
    SoftRenderer::offscreen_with_fonts(width, height, scale, FontSource::BundledOnly).unwrap()
}
fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
    Rect { x, y, width, height }
}
fn pixel(renderer: &SoftRenderer, x: u32, y: u32) -> [u8; 4] {
    let (width, _, data) = renderer.frame_rgba().unwrap();
    let at = ((y * width + x) * 4) as usize;
    data[at..at + 4].try_into().unwrap()
}
/// Pixel coordinates (x, y) whose colour satisfies `test`.
fn pixels_where(renderer: &SoftRenderer, test: impl Fn([u8; 4]) -> bool) -> Vec<(u32, u32)> {
    let (width, _, data) = renderer.frame_rgba().unwrap();
    data.as_chunks::<4>()
        .0
        .iter()
        .enumerate()
        .filter(|(_, p)| test(**p))
        .map(|(index, _)| (index as u32 % width, index as u32 / width))
        .collect()
}
const BLACK: Color = Color(0x000000);
const RED: [u8; 4] = [255, 0, 0, 255];

#[test]
fn nested_clips_restrict_fills_and_pops_restore_them() {
    let mut r = renderer(100, 100, 1.0);
    let ops = [
        DrawOp::Fill(rect(0.0, 0.0, 100.0, 100.0), BLACK),
        DrawOp::PushClip(rect(10.0, 10.0, 60.0, 60.0)),
        DrawOp::PushClip(rect(40.0, 0.0, 60.0, 50.0)),
        DrawOp::Fill(rect(0.0, 0.0, 100.0, 100.0), Color(0xFF0000)),
        DrawOp::PopClip,
        DrawOp::Fill(rect(0.0, 90.0, 100.0, 100.0), Color(0x00FF00)),
        DrawOp::PopClip,
        DrawOp::Fill(rect(95.0, 95.0, 5.0, 5.0), Color(0x0000FF)),
    ];
    assert_eq!(r.render(&ops).unwrap(), FrameStatus::Presented);
    let red = pixels_where(&r, |p| p == RED);
    assert_eq!(red.len(), 30 * 40, "only the 40..70 x 10..50 intersection");
    assert!(red.iter().all(|(x, y)| (40..70).contains(x) && (10..50).contains(y)));
    assert!(
        pixels_where(&r, |p| p == [0, 255, 0, 255]).is_empty(),
        "green lies outside the outer clip"
    );
    assert_eq!(pixel(&r, 97, 97), [0, 0, 255, 255], "pops restore the full frame");
    // Fractional scale: the clip edges round to whole pixels.
    let mut r = renderer(150, 150, 1.5);
    r.render(&ops).unwrap();
    let red = pixels_where(&r, |p| p == RED);
    assert!(red.iter().all(|(x, y)| (60..105).contains(x) && (15..75).contains(y)));
    assert_eq!(red.len(), 45 * 60);
}

#[test]
fn layers_composite_at_their_opacity_within_their_bounds() {
    let mut r = renderer(100, 60, 1.0);
    r.render(&[
        DrawOp::Fill(rect(0.0, 0.0, 100.0, 60.0), BLACK),
        DrawOp::PushLayer {
            bounds: rect(10.0, 10.0, 40.0, 40.0),
            opacity: 0.5,
        },
        DrawOp::Fill(rect(0.0, 0.0, 100.0, 60.0), Color(0xFFFFFF)),
        DrawOp::PushLayer {
            bounds: rect(20.0, 20.0, 80.0, 10.0),
            opacity: 1.0,
        },
        DrawOp::Fill(rect(0.0, 0.0, 100.0, 60.0), Color(0xFF0000)),
        DrawOp::PopLayer,
        DrawOp::PopLayer,
        DrawOp::PushLayer {
            bounds: rect(60.0, 10.0, 0.0, 0.0),
            opacity: 1.0,
        },
        DrawOp::Fill(rect(0.0, 0.0, 100.0, 60.0), Color(0x00FF00)),
        DrawOp::PopLayer,
    ])
    .unwrap();
    let grey = pixel(&r, 15, 15);
    assert!(
        grey[..3].iter().all(|c| (126..=129).contains(c)) && grey[3] == 255,
        "{grey:?}"
    );
    let dim_red = pixel(&r, 30, 25);
    assert!((126..=129).contains(&dim_red[0]) && dim_red[1] == 0, "{dim_red:?}");
    assert_eq!(
        pixel(&r, 55, 25),
        [0, 0, 0, 255],
        "the inner layer is clipped by the outer one"
    );
    assert_eq!(pixel(&r, 5, 5), [0, 0, 0, 255]);
    assert!(
        pixels_where(&r, |p| p[1] == 255).is_empty(),
        "an empty layer draws nothing"
    );
}

#[test]
fn images_scale_into_their_destination_with_opacity() {
    let mut r = renderer(80, 40, 1.0);
    let image = Image::rgba(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]).unwrap();
    r.render(&[
        DrawOp::Fill(rect(0.0, 0.0, 80.0, 40.0), BLACK),
        DrawOp::Image {
            image: image.clone(),
            destination: rect(0.0, 0.0, 40.0, 20.0),
            opacity: 1.0,
        },
        DrawOp::PushClip(rect(40.0, 20.0, 40.0, 20.0)),
        DrawOp::Image {
            image,
            destination: rect(20.0, 10.0, 60.0, 30.0),
            opacity: 0.5,
        },
        DrawOp::PopClip,
    ])
    .unwrap();
    assert_eq!(pixel(&r, 2, 10), RED);
    assert_eq!(pixel(&r, 37, 10), [0, 0, 255, 255]);
    let faded = pixel(&r, 75, 35);
    assert!(faded[2] > 100 && faded[2] < 140 && faded[0] < 10, "{faded:?}");
    assert_eq!(
        pixel(&r, 30, 30),
        [0, 0, 0, 255],
        "the clip hides the second image here"
    );
}

#[test]
fn rounded_rects_lines_and_strokes_cover_their_outlines() {
    let mut r = renderer(100, 100, 2.0);
    r.render(&[
        DrawOp::Fill(rect(0.0, 0.0, 50.0, 50.0), BLACK),
        DrawOp::FillRounded(rect(5.0, 5.0, 20.0, 20.0), Color(0xFF0000), 6.0),
        DrawOp::StrokeRounded(rect(28.0, 5.0, 18.0, 18.0), Color(0x00FF00), 4.0, 2.0),
        DrawOp::Stroke(rect(5.0, 30.0, 15.0, 15.0), Color(0x0000FF), 1.0),
        DrawOp::Line {
            from: Point { x: 25.0, y: 40.0 },
            to: Point { x: 48.0, y: 40.0 },
            color: Color(0xFFFFFF),
            width: 1.0,
        },
    ])
    .unwrap();
    assert_eq!(pixel(&r, 30, 30), RED, "inside the rounded fill");
    assert_eq!(pixel(&r, 11, 11), [0, 0, 0, 255], "the rounded corner is cut");
    assert_eq!(pixel(&r, 74, 10), [0, 255, 0, 255], "top edge of the rounded stroke");
    assert_eq!(pixel(&r, 74, 25), [0, 0, 0, 255], "the stroke is hollow");
    assert_eq!(
        pixel(&r, 20, 60),
        [0, 0, 255, 255],
        "rectangle stroke centred on its edge"
    );
    assert_eq!(pixel(&r, 70, 80), [255, 255, 255, 255]);
    assert_eq!(pixel(&r, 70, 84), [0, 0, 0, 255], "a 1 DIP line is 2 pixels at scale 2");
}

fn styled_frame(r: &mut SoftRenderer, id: LayoutId) -> Vec<(u32, u32)> {
    r.render(&[
        DrawOp::Fill(rect(0.0, 0.0, 300.0, 40.0), BLACK),
        DrawOp::Layout {
            origin: Point { x: 10.0, y: 8.0 },
            layout: id,
            color: Color(0xFFFFFF),
        },
    ])
    .unwrap();
    pixels_where(r, |p| p[0] > 150 && p[1] < 60 && p[2] < 60)
}

#[test]
fn styles_colour_only_their_byte_ranges_and_keep_geometry() {
    let mut r = renderer(300, 40, 1.0);
    let text = "plain 文字 keyword tail";
    let id = r.shape(text, 16.0, 280.0).unwrap();
    assert!(styled_frame(&mut r, id).is_empty(), "unstyled text is white");
    let plain = r.frame_rgba().unwrap().2.to_vec();
    let start = text.find("keyword").unwrap();
    let end = start + "keyword".len();
    let caret = r.caret(id, start).unwrap();
    r.set_styles(
        id,
        &[TextStyle {
            bytes: start..end,
            color: Color(0xFF0000),
        }],
    )
    .unwrap();
    assert_eq!(r.caret(id, start).unwrap(), caret, "styles never change geometry");
    let red = styled_frame(&mut r, id);
    assert!(red.len() > 40, "the keyword is red");
    let (left, right) = (10.0 + caret.x, 10.0 + r.caret(id, end).unwrap().x);
    assert!(
        red.iter()
            .all(|(x, _)| *x as f32 >= left - 1.0 && (*x as f32) < right + 1.0),
        "red stays within {left}..{right}"
    );
    let white = pixels_where(&r, |p| p[0] > 200 && p[1] > 200 && p[2] > 200);
    assert!(white.iter().any(|(x, _)| (*x as f32) < left) && white.iter().any(|(x, _)| (*x as f32) > right));
    assert!(
        white
            .iter()
            .all(|(x, _)| (*x as f32) < left + 1.0 || (*x as f32) > right - 1.0)
    );
    r.set_styles(id, &[]).unwrap();
    styled_frame(&mut r, id);
    assert_eq!(
        r.frame_rgba().unwrap().2,
        plain.as_slice(),
        "clearing styles restores the frame"
    );
}

#[test]
fn invalid_frames_fail_before_drawing_anything() {
    let mut r = renderer(40, 40, 1.0);
    assert!(r.frame_rgba().is_none());
    assert!(r.write_png(std::env::temp_dir().join("never.png")).is_err());
    r.render(&[DrawOp::Fill(rect(0.0, 0.0, 40.0, 40.0), Color(0x123456))])
        .unwrap();
    let before = r.frame_rgba().unwrap().2.to_vec();
    let crossed = [
        DrawOp::PushClip(rect(0.0, 0.0, 1.0, 1.0)),
        DrawOp::PushLayer {
            bounds: rect(0.0, 0.0, 1.0, 1.0),
            opacity: 1.0,
        },
        DrawOp::PopClip,
        DrawOp::PopLayer,
    ];
    assert!(matches!(r.render(&crossed), Err(SoftError::InvalidOperations)));
    assert!(matches!(
        r.render(&[DrawOp::PopClip]),
        Err(SoftError::InvalidOperations)
    ));
    let id = r.shape("gone", 13.0, 100.0).unwrap();
    r.release_layout(id);
    let stale = [
        DrawOp::Fill(rect(0.0, 0.0, 40.0, 40.0), BLACK),
        DrawOp::Layout {
            origin: Point::default(),
            layout: id,
            color: BLACK,
        },
    ];
    assert!(matches!(
        r.render(&stale),
        Err(SoftError::Layout(LayoutError::InvalidHandle))
    ));
    let bad_text = DrawOp::Text {
        origin: Point::default(),
        text: "x".into(),
        size: f32::NAN,
        color: BLACK,
    };
    assert!(matches!(r.render(&[bad_text]), Err(SoftError::InvalidOperations)));
    assert_eq!(r.frame_rgba().unwrap().2, before.as_slice());
    for (width, height, scale) in [(0, 10, 1.0), (10, 10, 0.25), (10, 10, f32::NAN), (5000, 5000, 1.0)] {
        assert!(matches!(
            SoftRenderer::offscreen_with_fonts(width, height, scale, FontSource::BundledOnly),
            Err(SoftError::InvalidSize)
        ));
    }
    assert!(matches!(r.resize(10, 10, 0.0), Err(SoftError::InvalidSize)));
}

#[test]
fn text_is_clipped_to_its_line_box_and_resizes_take_effect() {
    let mut r = renderer(200, 60, 1.0);
    let label = |y: f32| DrawOp::Text {
        origin: Point { x: 4.0, y },
        text: "Ag\nsecond line".into(),
        size: 13.0,
        color: Color(0xFFFFFF),
    };
    r.render(&[DrawOp::Fill(rect(0.0, 0.0, 200.0, 60.0), BLACK), label(4.0)])
        .unwrap();
    let lit = pixels_where(&r, |p| p[0] > 100);
    assert!(!lit.is_empty());
    assert!(
        lit.iter().all(|(_, y)| (*y as f32) < 4.0 + 13.0 * 1.8),
        "only the first line fits the 1.8 em box"
    );
    r.resize(400, 120, 2.0).unwrap();
    r.render(&[DrawOp::Fill(rect(0.0, 0.0, 200.0, 60.0), BLACK), label(4.0)])
        .unwrap();
    let (width, height, _) = r.frame_rgba().unwrap();
    assert_eq!((width, height), (400, 120));
    assert_eq!(r.size(), (400, 120, 2.0));
    let scaled = pixels_where(&r, |p| p[0] > 100);
    assert!(scaled.len() > lit.len() * 3, "glyphs rasterize at the new scale");
}

#[test]
fn the_shared_shell_renders_a_non_blank_frame_and_saves_a_png() {
    let mut r = renderer(640, 480, 1.0);
    let ops = bareline_ui::shell(640.0, 480.0, &["Untitled".into()], 0, false);
    assert_eq!(r.render(&ops).unwrap(), FrameStatus::Presented);
    let (_, _, data) = r.frame_rgba().unwrap();
    let mut colours: Vec<[u8; 4]> = data.as_chunks::<4>().0.to_vec();
    assert!(colours.iter().all(|p| p[3] == 255), "the shell covers the whole frame");
    colours.sort_unstable();
    colours.dedup();
    assert!(
        colours.len() > 20,
        "chrome, editor and antialiased text: {}",
        colours.len()
    );
    // The tab title is drawn in the tab strip, left of the editor gutter.
    let theme_text = pixels_where(&r, |p| p[0] > 150 && p[1] > 150 && p[2] > 150);
    assert!(theme_text.iter().any(|(x, y)| *x < 200 && *y < 34), "tab title ink");
    let path = std::env::temp_dir().join(format!("bareline-soft-shell-{}.png", std::process::id()));
    r.write_png(&path).unwrap();
    let decoded = tiny_skia::Pixmap::load_png(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(decoded.data(), data, "the PNG holds the frame exactly");
}
