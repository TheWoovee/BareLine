// SPDX-License-Identifier: MPL-2.0
//! `TextBackend` geometry on the bundled face: carets, hit tests and range
//! rectangles agree with each other on ASCII, CJK, combining and RTL text.
use bareline_renderer::{LayoutError, LayoutId, Point, TextBackend, TextStyle};
use bareline_renderer_soft::{BUNDLED_FONT_FAMILY, FontSource, SoftRenderer};

fn renderer(scale: f32) -> SoftRenderer {
    SoftRenderer::offscreen_with_fonts(320, 120, scale, FontSource::BundledOnly).unwrap()
}
fn boundaries(text: &str) -> Vec<usize> {
    text.char_indices()
        .map(|(index, _)| index)
        .chain(Some(text.len()))
        .collect()
}
/// Every caret lies on the first line, and clicking just right of a caret's
/// leading edge returns that caret's offset, except inside clusters.
fn assert_round_trips(backend: &mut SoftRenderer, id: LayoutId, text: &str, grapheme_starts: &[usize]) {
    let height = backend.layout_size(id).unwrap().1;
    for offset in grapheme_starts.iter().copied().filter(|offset| *offset < text.len()) {
        let caret = backend.caret(id, offset).unwrap();
        assert_eq!(caret.y, 0.0);
        assert!(caret.height > 0.0 && caret.height <= height + 0.01);
        let hit = backend
            .hit_test(
                id,
                Point {
                    x: caret.x + 0.5,
                    y: caret.height / 2.0,
                },
            )
            .unwrap();
        assert_eq!(hit.byte_offset, offset, "{text:?} at {offset}");
        assert!(hit.inside && !hit.trailing);
    }
}

#[test]
fn ascii_carets_advance_evenly_and_hit_tests_round_trip() {
    let mut backend = renderer(1.0);
    let text = "let x = 10;";
    let id = backend.shape(text, 16.0, 300.0).unwrap();
    let carets: Vec<f32> = (0..=text.len()).map(|i| backend.caret(id, i).unwrap().x).collect();
    let advance = carets[1] - carets[0];
    assert!(
        (9.0..10.5).contains(&advance),
        "DejaVu Sans Mono advance at 16 px: {advance}"
    );
    for pair in carets.windows(2) {
        assert!(
            (pair[1] - pair[0] - advance).abs() < 0.01,
            "monospace carets {carets:?}"
        );
    }
    let (width, height) = backend.layout_size(id).unwrap();
    assert!(
        (width - carets[text.len()]).abs() < 0.01,
        "width includes the whole line"
    );
    assert!((16.0..24.0).contains(&height));
    assert_round_trips(&mut backend, id, text, &boundaries(text));
    let rects = backend.range_rects(id, 4..9).unwrap();
    assert_eq!(rects.len(), 1);
    assert!((rects[0].x - carets[4]).abs() < 0.01 && (rects[0].width - (carets[9] - carets[4])).abs() < 0.01);
    assert_eq!(backend.range_rects(id, 3..3).unwrap(), []);
    // Past either end of the line: outside, on the nearest edge.
    let left = backend.hit_test(id, Point { x: -5.0, y: 4.0 }).unwrap();
    assert_eq!((left.byte_offset, left.inside, left.trailing), (0, false, false));
    let right = backend.hit_test(id, Point { x: 500.0, y: 4.0 }).unwrap();
    assert_eq!(
        (right.byte_offset, right.inside, right.trailing),
        (text.len(), false, true)
    );
    let trailing = backend
        .hit_test(
            id,
            Point {
                x: carets[1] - 0.5,
                y: 4.0,
            },
        )
        .unwrap();
    assert_eq!((trailing.byte_offset, trailing.trailing), (1, true));
    // A trailing space counts toward the width, as `measured_columns` needs.
    let space = backend
        .shape_with_font_family(" ", 16.0, 1.0e6, BUNDLED_FONT_FAMILY)
        .unwrap();
    assert!((backend.layout_size(space).unwrap().0 - advance).abs() < 0.01);
}

#[test]
fn cjk_and_combining_text_snap_to_graphemes_and_reject_split_characters() {
    let mut backend = renderer(1.0);
    let text = "文字ab";
    let id = backend.shape(text, 16.0, 300.0).unwrap();
    assert_round_trips(&mut backend, id, text, &boundaries(text));
    assert_eq!(backend.caret(id, 1), Err(LayoutError::InvalidOffset));
    assert_eq!(backend.range_rects(id, 0..4), Err(LayoutError::InvalidOffset));
    assert_eq!(backend.caret(id, text.len() + 1), Err(LayoutError::InvalidOffset));
    let wide = backend.range_rects(id, 0..3).unwrap();
    assert!(wide.len() == 1 && wide[0].width > 0.0);
    // Every hit test lands on a character boundary.
    for x in 0..80 {
        let hit = backend.hit_test(id, Point { x: x as f32, y: 8.0 }).unwrap();
        assert!(text.is_char_boundary(hit.byte_offset));
    }

    let text = "e\u{301}x\u{0323}\u{0307}z";
    let id = backend.shape(text, 16.0, 300.0).unwrap();
    let starts = [0, 3, 8];
    assert_round_trips(&mut backend, id, text, &starts);
    for x in 0..60 {
        let hit = backend.hit_test(id, Point { x: x as f32, y: 8.0 }).unwrap();
        assert!(
            [0, 3, 8, text.len()].contains(&hit.byte_offset),
            "hit inside a cluster: {hit:?}"
        );
    }
    // A mark's own offset is a valid character boundary; its caret is the cluster's leading edge.
    assert_eq!(backend.caret(id, 1).unwrap().x, backend.caret(id, 0).unwrap().x);
    let accent = backend.range_rects(id, 0..3).unwrap();
    let next = backend.caret(id, 3).unwrap().x;
    assert!(accent.len() == 1 && (accent[0].width - next).abs() < 0.01);
}

#[test]
fn arabic_logical_order_moves_visually_left() {
    let mut backend = renderer(1.0);
    let text = "Latin مرحبا end";
    let id = backend.shape(text, 16.0, 700.0).unwrap();
    let arabic = text.find('م').unwrap();
    let a = backend.caret(id, arabic).unwrap();
    let b = backend.caret(id, arabic + 'م'.len_utf8()).unwrap();
    assert!(b.x < a.x, "RTL advance must move left: {a:?} {b:?}");
    assert_eq!(backend.caret(id, arabic + 1), Err(LayoutError::InvalidOffset));
    assert!(!backend.range_rects(id, arabic..text.len()).unwrap().is_empty());
    for x in (0..200).step_by(3) {
        let hit = backend.hit_test(id, Point { x: x as f32, y: 8.0 }).unwrap();
        assert!(text.is_char_boundary(hit.byte_offset));
    }
}

#[test]
fn wrapped_layouts_grow_taller_as_they_narrow_and_keep_carets_inside() {
    let mut backend = renderer(1.0);
    let text = "The quick brown fox jumps over the lazy dog and keeps running far beyond the edge.";
    let wide = backend.shape_wrapped(text, 14.0, 900.0, BUNDLED_FONT_FAMILY).unwrap();
    let narrow = backend.shape_wrapped(text, 14.0, 160.0, BUNDLED_FONT_FAMILY).unwrap();
    let (wide_width, wide_height) = backend.layout_size(wide).unwrap();
    let (narrow_width, narrow_height) = backend.layout_size(narrow).unwrap();
    assert!(
        (wide_height - 14.0 * 1.2).abs() < 0.01,
        "one uniform line: {wide_height}"
    );
    assert!(narrow_height >= wide_height * 4.0, "{narrow_height} vs {wide_height}");
    assert!(narrow_width <= 160.0 + 9.0 && narrow_width < wide_width);
    let last = backend.caret(narrow, text.len()).unwrap();
    assert!(last.y > 0.0 && last.y + last.height <= narrow_height + 0.01);
    let below = backend
        .hit_test(
            narrow,
            Point {
                x: 1.0,
                y: narrow_height + 50.0,
            },
        )
        .unwrap();
    assert!(!below.inside && below.byte_offset > 0);
    let rects = backend.range_rects(narrow, 0..text.len()).unwrap();
    assert!(rects.len() >= 4, "one rectangle per wrapped line: {rects:?}");
    // Explicit line breaks start new lines too.
    let lines = backend.shape("a\nb\r\nc", 14.0, 300.0).unwrap();
    assert_eq!(
        backend.caret(lines, 2).unwrap().y,
        backend.caret(lines, 0).unwrap().height
    );
    assert!(backend.caret(lines, 5).unwrap().y > backend.caret(lines, 2).unwrap().y);
}

#[test]
fn geometry_is_in_dips_at_every_scale_and_handles_are_checked() {
    let mut reference = renderer(1.0);
    let text = "Bareline 文 e\u{301}";
    let base = reference.shape(text, 13.0, 400.0).unwrap();
    for scale in [1.5, 2.0] {
        let mut scaled = renderer(scale);
        let id = scaled.shape(text, 13.0, 400.0).unwrap();
        assert_eq!(scaled.layout_size(id), reference.layout_size(base));
        for offset in boundaries(text) {
            assert_eq!(scaled.caret(id, offset), reference.caret(base, offset));
        }
    }
    let measured = reference.measure_text(text, 13.0).unwrap();
    assert_eq!(measured, reference.layout_size(base).unwrap());
    reference.release_layout(base);
    assert_eq!(reference.caret(base, 0), Err(LayoutError::InvalidHandle));
    assert_eq!(reference.layout_size(base), Err(LayoutError::InvalidHandle));
    assert_eq!(
        reference.set_styles(
            base,
            &[TextStyle {
                bytes: 0..1,
                color: bareline_renderer::Color(0)
            }]
        ),
        Err(LayoutError::InvalidHandle)
    );
    assert_eq!(reference.layout_count(), 0);
    let crossed = std::ops::Range { start: 3, end: 1 };
    assert_eq!(reference.range_rects(base, crossed), Err(LayoutError::InvalidOffset));
    assert_eq!(
        reference.shape_with_font_family("x", 13.0, 10.0, " "),
        Err(LayoutError::InvalidOffset)
    );
    for (size, width) in [(0.0, 10.0), (f32::NAN, 10.0), (13.0, 0.0), (13.0, f32::INFINITY)] {
        assert_eq!(reference.shape("x", size, width), Err(LayoutError::ResourceLimit));
    }
    let huge = "x".repeat(bareline_renderer::MAX_LAYOUT_BYTES + 1);
    assert_eq!(reference.shape(&huge, 13.0, 10.0), Err(LayoutError::ResourceLimit));
}
