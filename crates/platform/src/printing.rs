// SPDX-License-Identifier: MPL-2.0
//! OS-neutral, bounded line-at-a-time print contract. Drivers and dialogs stay platform-owned.
use std::{ops::Range, sync::atomic::AtomicBool};
#[derive(Clone, Debug, PartialEq)]
pub struct PrintOptions {
    pub title: String,
    pub font_family: String,
    pub font_size_pt: f64,
    pub margin_mm: f64,
    pub line_numbers: bool,
    pub header: bool,
    pub footer: bool,
    pub syntax_colors: bool,
    pub foreground: u32,
    pub background: u32,
    pub tab_width: u8,
}
impl Default for PrintOptions {
    fn default() -> Self {
        Self {
            title: "Bareline document".into(),
            font_family: "Consolas".into(),
            font_size_pt: 10.0,
            margin_mm: 12.0,
            line_numbers: true,
            header: true,
            footer: true,
            syntax_colors: true,
            foreground: 0,
            background: 0xffffff,
            tab_width: 4,
        }
    }
}
impl PrintOptions {
    pub fn validate(&self) -> Result<(), PrintError> {
        if self.title.len() > 4096
            || self.font_family.len() > 256
            || self.title.contains('\0')
            || self.font_family.contains('\0')
            || !self.font_size_pt.is_finite()
            || !(6.0..=72.0).contains(&self.font_size_pt)
            || !self.margin_mm.is_finite()
            || !(0.0..=75.0).contains(&self.margin_mm)
            || !(1..=16).contains(&self.tab_width)
            || self.foreground > 0xffffff
            || self.background > 0xffffff
        {
            return Err(PrintError::InvalidOptions);
        }
        Ok(())
    }
}
#[derive(Clone, Debug)]
pub struct PrintSpan {
    pub bytes: Range<usize>,
    pub rgb: u32,
}
pub struct PrintLine<'a> {
    pub number: usize,
    pub text: &'a str,
    pub spans: &'a [PrintSpan],
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrintError {
    Cancelled,
    Unavailable(String),
    Driver(String),
    InvalidOptions,
    InvalidLine,
}
/// Plain-language reason shown to the user (UI-03); `Debug` stays for diagnostics.
/// Platform printers already word `Unavailable` and `Driver` for the user.
impl std::fmt::Display for PrintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("Printing was cancelled"),
            Self::Unavailable(reason) | Self::Driver(reason) => f.write_str(reason),
            Self::InvalidOptions => f.write_str("The print options are not valid; check the margins and page setup"),
            Self::InvalidLine => f.write_str("A line could not be prepared for printing"),
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct PrintSummary {
    pub pages: u32,
    pub lines: u64,
}
/// Nominal page the on-screen preview is laid out on: US Letter, in points.
/// A printer reports its own geometry; the composition rules are the same.
pub const PREVIEW_PAGE_POINTS: (f64, f64) = (612.0, 792.0);
/// Advance of a monospace glyph relative to its em size.
const MONOSPACE_ADVANCE: f64 = 0.6;
/// Width of the line-number gutter, `{:>6}` plus two spaces, in characters.
pub const LINE_NUMBER_COLUMNS: usize = 8;
/// One printed row of a preview page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewRow {
    /// Source line number on the first row of a line; `None` on the rows a
    /// long line wraps onto.
    pub number: Option<usize>,
    pub text: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreviewPage {
    pub rows: Vec<PreviewRow>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrintPreview {
    pub pages: Vec<PreviewPage>,
    /// Text columns and body rows of one page.
    pub columns: usize,
    pub rows_per_page: usize,
    /// Printing continues past the last page laid out here.
    pub truncated: bool,
}
/// Page body geometry for `options` on the nominal page, following the same
/// rules as the print job: a header takes two rows, the bottom two rows are
/// kept for the footer, and numbered rows lose the gutter width.
pub fn preview_geometry(options: &PrintOptions) -> (usize, usize) {
    let (width, height) = PREVIEW_PAGE_POINTS;
    let size = if options.font_size_pt.is_finite() {
        options.font_size_pt.clamp(6.0, 72.0)
    } else {
        10.0
    };
    let margin = if options.margin_mm.is_finite() {
        (options.margin_mm.clamp(0.0, 75.0) * 72.0 / 25.4).round()
    } else {
        0.0
    };
    let line_height = (size * 1.25).ceil();
    let top = margin + if options.header { line_height * 2.0 } else { 0.0 };
    let rows = ((height - margin - line_height * 2.0 - top) / line_height)
        .floor()
        .max(1.0) as usize;
    let gutter = if options.line_numbers { LINE_NUMBER_COLUMNS } else { 0 };
    let columns = (((width - 2.0 * margin) / (size * MONOSPACE_ADVANCE)).floor().max(0.0) as usize)
        .saturating_sub(gutter)
        .max(1);
    (columns, rows)
}
/// Lay `lines` (number, text) out into at most `max_pages` pages the way the
/// print job does: tabs expanded, trailing line breaks dropped, long lines
/// wrapped after the last space that fits.
pub fn paginate_preview<'a>(
    lines: impl IntoIterator<Item = (usize, &'a str)>,
    options: &PrintOptions,
    max_pages: usize,
) -> PrintPreview {
    let (columns, rows_per_page) = preview_geometry(options);
    let mut preview = PrintPreview {
        pages: vec![PreviewPage::default()],
        columns,
        rows_per_page,
        truncated: false,
    };
    let tab_width = usize::from(options.tab_width.clamp(1, 16));
    for (number, line) in lines {
        let mut expanded = String::new();
        let mut column = 0;
        for c in line.trim_end_matches(['\r', '\n']).chars() {
            if c == '\t' {
                let spaces = tab_width - column % tab_width;
                expanded.extend(std::iter::repeat_n(' ', spaces));
                column += spaces;
            } else {
                expanded.push(c);
                column = if matches!(c, '\r' | '\n') { 0 } else { column + 1 };
            }
        }
        let mut rest = expanded.as_str();
        let mut first = true;
        loop {
            let mut take = rest.char_indices().nth(columns).map_or(rest.len(), |(at, _)| at);
            if take < rest.len()
                && let Some((space, c)) = rest[..take].char_indices().rev().find(|(_, c)| c.is_whitespace())
                && space > 0
            {
                take = space + c.len_utf8();
            }
            let page = preview.pages.last_mut().expect("at least one page");
            if page.rows.len() == rows_per_page {
                if preview.pages.len() == max_pages.max(1) {
                    preview.truncated = true;
                    return preview;
                }
                preview.pages.push(PreviewPage::default());
            }
            preview
                .pages
                .last_mut()
                .expect("at least one page")
                .rows
                .push(PreviewRow {
                    number: first.then_some(number),
                    text: rest[..take].to_owned(),
                });
            rest = &rest[take..];
            first = false;
            if rest.is_empty() {
                break;
            }
        }
    }
    preview
}
pub trait PrintTarget {
    /// Maximum logical line length is 256 KiB. Call only on a bounded print worker.
    fn write_line(&mut self, line: PrintLine<'_>, cancel: &AtomicBool) -> Result<(), PrintError>;
    fn finish(self: Box<Self>, cancel: &AtomicBool) -> Result<PrintSummary, PrintError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    fn options(line_numbers: bool, header: bool) -> PrintOptions {
        PrintOptions {
            line_numbers,
            header,
            ..PrintOptions::default()
        }
    }
    #[test]
    fn preview_pages_the_document_text_not_a_sample() {
        let text: Vec<String> = (1..=150).map(|n| format!("line {n}")).collect();
        let preview = paginate_preview(
            text.iter().enumerate().map(|(i, line)| (i + 1, line.as_str())),
            &options(true, true),
            10,
        );
        let rows: usize = preview.pages.iter().map(|page| page.rows.len()).sum();
        assert_eq!(rows, 150);
        assert!(preview.pages.len() > 1, "{} rows per page", preview.rows_per_page);
        assert!(!preview.truncated);
        assert_eq!(preview.pages[0].rows.len(), preview.rows_per_page);
        assert_eq!(
            preview.pages[0].rows[0],
            PreviewRow {
                number: Some(1),
                text: "line 1".into()
            }
        );
        let second = &preview.pages[1].rows[0];
        assert_eq!(second.number, Some(preview.rows_per_page + 1));
    }
    #[test]
    fn preview_wraps_expands_tabs_and_stops_at_the_page_bound() {
        let plain = options(false, false);
        let (columns, rows) = preview_geometry(&plain);
        let numbered = preview_geometry(&options(true, true));
        assert_eq!(numbered.0, columns - LINE_NUMBER_COLUMNS);
        assert_eq!(numbered.1, rows - 2, "the header takes two rows");
        let long = format!("{} tail", "x".repeat(columns - 2));
        let preview = paginate_preview([(7, long.as_str()), (8, "\tz\r\n")], &plain, 1);
        let page = &preview.pages[0];
        assert_eq!(page.rows[0].text, format!("{} ", "x".repeat(columns - 2)));
        assert_eq!(
            page.rows[1],
            PreviewRow {
                number: None,
                text: "tail".into()
            }
        );
        assert_eq!(
            page.rows[2],
            PreviewRow {
                number: Some(8),
                text: "    z".into()
            }
        );
        let many = vec!["row"; rows * 3];
        let bounded = paginate_preview(many.iter().enumerate().map(|(i, s)| (i + 1, *s)), &plain, 2);
        assert_eq!(bounded.pages.len(), 2);
        assert!(bounded.truncated);
        let empty = paginate_preview(std::iter::empty(), &plain, 3);
        assert_eq!(empty.pages.len(), 1);
        assert!(empty.pages[0].rows.is_empty());
    }
}
