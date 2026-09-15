// SPDX-License-Identifier: MPL-2.0
//! Native printer selection and a cancellable GDI spool job. No document or filesystem writes.
use bareline_platform::printing::{PrintError, PrintLine, PrintOptions, PrintSpan, PrintSummary, PrintTarget};
use std::sync::atomic::{AtomicBool, Ordering};
use windows::{
    Win32::{
        Foundation::{COLORREF, HGLOBAL, RECT},
        Graphics::Gdi::*,
        Storage::Xps::*,
        System::Memory::{GlobalLock, GlobalSize, GlobalUnlock},
        UI::Controls::Dialogs::*,
    },
    core::{PCWSTR, w},
};

pub struct PrinterSelection {
    name: Vec<u16>,
    mode: Vec<u32>,
}
/// User-triggered native dialog; returns owned device configuration for the print worker.
pub fn choose_printer() -> Result<Option<PrinterSelection>, PrintError> {
    let mut dialog = PRINTDLGW {
        lStructSize: std::mem::size_of::<PRINTDLGW>() as u32,
        Flags: PD_RETURNDC | PD_NOPAGENUMS | PD_NOSELECTION | PD_HIDEPRINTTOFILE,
        nCopies: 1,
        ..Default::default()
    };
    // SAFETY: the initialized structure remains live for the modal call.
    let selected = unsafe { PrintDlgW(&mut dialog).as_bool() };
    let result = (|| {
        if !selected {
            let code = unsafe { CommDlgExtendedError() }.0;
            return if code == 0 {
                Ok(None)
            } else {
                Err(PrintError::Unavailable(format!(
                    "Printer selection failed ({code}); select another printer and retry"
                )))
            };
        }
        let names = copy_global(dialog.hDevNames)?;
        if names.len() < 8 {
            return Err(PrintError::Unavailable("Printer did not provide its name".into()));
        }
        let offset = u16::from_le_bytes([names[2], names[3]]) as usize * 2;
        if offset >= names.len() || names.len() % 2 != 0 {
            return Err(PrintError::InvalidOptions);
        }
        let mut name: Vec<u16> = names[offset..]
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .take_while(|v| *v != 0)
            .collect();
        if name.is_empty() || name.len() > 4096 {
            return Err(PrintError::InvalidOptions);
        }
        name.push(0);
        let bytes = copy_global(dialog.hDevMode)?;
        if bytes.len() < std::mem::size_of::<DEVMODEW>() {
            return Err(PrintError::InvalidOptions);
        }
        let mut mode = vec![0u32; bytes.len().div_ceil(4)];
        // SAFETY: u32 storage supplies DEVMODE alignment; byte view is within its allocation.
        (unsafe { std::slice::from_raw_parts_mut(mode.as_mut_ptr().cast::<u8>(), mode.len() * 4) })[..bytes.len()]
            .copy_from_slice(&bytes);
        let header = unsafe { &*mode.as_ptr().cast::<DEVMODEW>() };
        if (header.dmSize as usize) < std::mem::size_of::<DEVMODEW>()
            || (header.dmSize as usize).saturating_add(header.dmDriverExtra as usize) > bytes.len()
        {
            return Err(PrintError::InvalidOptions);
        }
        Ok(Some(PrinterSelection { name, mode }))
    })();
    unsafe {
        if !dialog.hDC.is_invalid() {
            let _ = DeleteDC(dialog.hDC);
        }
        if !dialog.hDevMode.is_invalid() {
            let _ = windows::Win32::Foundation::GlobalFree(Some(dialog.hDevMode));
        }
        if !dialog.hDevNames.is_invalid() {
            let _ = windows::Win32::Foundation::GlobalFree(Some(dialog.hDevNames));
        }
    }
    result
}
fn copy_global(handle: HGLOBAL) -> Result<Vec<u8>, PrintError> {
    if handle.is_invalid() {
        return Err(PrintError::InvalidOptions);
    }
    let length = unsafe { GlobalSize(handle) };
    if length == 0 || length > 65536 {
        return Err(PrintError::InvalidOptions);
    }
    let pointer = unsafe { GlobalLock(handle) };
    if pointer.is_null() {
        return Err(PrintError::Unavailable(
            "Printer configuration could not be read".into(),
        ));
    }
    let bytes = unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), length) }.to_vec();
    let _ = unsafe { GlobalUnlock(handle) };
    Ok(bytes)
}
pub struct WindowsPrintJob {
    dc: HDC,
    font: HFONT,
    old_font: HGDIOBJ,
    options: PrintOptions,
    width: i32,
    height: i32,
    margin_x: i32,
    margin_y: i32,
    line_height: i32,
    y: i32,
    page_open: bool,
    finished: bool,
    summary: PrintSummary,
}
impl WindowsPrintJob {
    /// Construct and consume on the same worker. RAII aborts incomplete jobs.
    pub fn start(selection: PrinterSelection, options: PrintOptions) -> Result<Self, PrintError> {
        options.validate()?;
        let dc = unsafe {
            CreateDCW(
                w!("WINSPOOL"),
                PCWSTR(selection.name.as_ptr()),
                PCWSTR::null(),
                Some(selection.mode.as_ptr().cast()),
            )
        };
        if dc.is_invalid() {
            return Err(PrintError::Unavailable(
                "Printer unavailable; select another printer and retry".into(),
            ));
        }
        let dpi_x = unsafe { GetDeviceCaps(Some(dc), LOGPIXELSX) };
        let dpi_y = unsafe { GetDeviceCaps(Some(dc), LOGPIXELSY) };
        let width = unsafe { GetDeviceCaps(Some(dc), HORZRES) };
        let height = unsafe { GetDeviceCaps(Some(dc), VERTRES) };
        let font_name: Vec<u16> = options.font_family.encode_utf16().chain(Some(0)).collect();
        let font = unsafe {
            CreateFontW(
                -(options.font_size_pt * dpi_y as f64 / 72.0).round() as i32,
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                DEFAULT_QUALITY,
                0,
                PCWSTR(font_name.as_ptr()),
            )
        };
        if font.is_invalid() || dpi_x <= 0 || dpi_y <= 0 || width <= 0 || height <= 0 {
            unsafe {
                if !font.is_invalid() {
                    let _ = DeleteObject(HGDIOBJ(font.0));
                }
                let _ = DeleteDC(dc);
            }
            return Err(PrintError::Unavailable(
                "Printer font or page geometry unavailable".into(),
            ));
        }
        let old_font = unsafe { SelectObject(dc, HGDIOBJ(font.0)) };
        let margin_x = (options.margin_mm * dpi_x as f64 / 25.4).round() as i32;
        let margin_y = (options.margin_mm * dpi_y as f64 / 25.4).round() as i32;
        let line_height = (options.font_size_pt * dpi_y as f64 / 72.0 * 1.25).ceil() as i32;
        let mut job = Self {
            dc,
            font,
            old_font,
            options,
            width,
            height,
            margin_x,
            margin_y,
            line_height,
            y: 0,
            page_open: false,
            finished: false,
            summary: Default::default(),
        };
        if width <= 2 * margin_x + line_height * 8 || height <= 2 * margin_y + line_height * 4 {
            return Err(PrintError::InvalidOptions);
        }
        let title: Vec<u16> = job.options.title.encode_utf16().chain(Some(0)).collect();
        let info = DOCINFOW {
            cbSize: std::mem::size_of::<DOCINFOW>() as i32,
            lpszDocName: PCWSTR(title.as_ptr()),
            ..Default::default()
        };
        if unsafe { StartDocW(dc, &info) } <= 0 {
            return Err(PrintError::Driver(
                "Printer rejected the job; retry with another printer".into(),
            ));
        }
        unsafe {
            SetBkMode(dc, TRANSPARENT);
        }
        job.next_page()?;
        Ok(job)
    }
    fn output(&self, x: i32, y: i32, text: &str, rgb: u32) -> Result<i32, PrintError> {
        let wide: Vec<u16> = text.encode_utf16().collect();
        let mut size = Default::default();
        unsafe {
            SetTextColor(
                self.dc,
                COLORREF(((rgb & 255) << 16) | (rgb & 0xff00) | ((rgb >> 16) & 255)),
            );
        }
        if !unsafe { TextOutW(self.dc, x, y, &wide) }.as_bool()
            || !unsafe { GetTextExtentPoint32W(self.dc, &wide, &mut size) }.as_bool()
        {
            return Err(PrintError::Driver(
                "Printer text output failed; retry the document".into(),
            ));
        }
        Ok(size.cx)
    }
    fn end_page(&mut self) -> Result<(), PrintError> {
        if self.page_open {
            if self.options.footer {
                self.output(
                    self.margin_x,
                    self.height - self.margin_y - self.line_height,
                    &format!("Page {}", self.summary.pages),
                    if self.options.syntax_colors {
                        self.options.foreground
                    } else {
                        0
                    },
                )?;
            }
            if unsafe { EndPage(self.dc) } <= 0 {
                return Err(PrintError::Driver("Printer could not complete the page".into()));
            }
            self.page_open = false;
        }
        Ok(())
    }
    fn next_page(&mut self) -> Result<(), PrintError> {
        self.end_page()?;
        if unsafe { StartPage(self.dc) } <= 0 {
            return Err(PrintError::Driver("Printer could not start a page".into()));
        }
        self.page_open = true;
        self.summary.pages += 1;
        self.y = self.margin_y;
        if self.options.syntax_colors {
            let rgb = self.options.background;
            let brush =
                unsafe { CreateSolidBrush(COLORREF(((rgb & 255) << 16) | (rgb & 0xff00) | ((rgb >> 16) & 255))) };
            if brush.is_invalid() {
                return Err(PrintError::Driver("Printer page background unavailable".into()));
            }
            let filled = unsafe {
                FillRect(
                    self.dc,
                    &RECT {
                        left: 0,
                        top: 0,
                        right: self.width,
                        bottom: self.height,
                    },
                    brush,
                )
            };
            unsafe {
                let _ = DeleteObject(HGDIOBJ(brush.0));
            }
            if filled == 0 {
                return Err(PrintError::Driver("Printer page background failed".into()));
            }
        }
        if self.options.header {
            self.output(
                self.margin_x,
                self.y,
                &self.options.title,
                if self.options.syntax_colors {
                    self.options.foreground
                } else {
                    0
                },
            )?;
            self.y += self.line_height * 2;
        }
        Ok(())
    }
}
impl PrintTarget for WindowsPrintJob {
    fn write_line(&mut self, line: PrintLine<'_>, cancel: &AtomicBool) -> Result<(), PrintError> {
        if line.text.len() > 256 * 1024
            || line.spans.windows(2).any(|s| s[0].bytes.end > s[1].bytes.start)
            || line.spans.iter().any(|s| {
                s.bytes.start > s.bytes.end
                    || s.bytes.end > line.text.len()
                    || !line.text.is_char_boundary(s.bytes.start)
                    || !line.text.is_char_boundary(s.bytes.end)
            })
        {
            return Err(PrintError::InvalidLine);
        }
        if line.text.contains('\t') {
            let (text, spans) = expand_tabs(&line, self.options.tab_width as usize)?;
            return self.write_line(
                PrintLine {
                    number: line.number,
                    text: &text,
                    spans: &spans,
                },
                cancel,
            );
        }
        let text = line.text.trim_end_matches(['\r', '\n']);
        let mut at = 0;
        let mut first = true;
        loop {
            if cancel.load(Ordering::Acquire) {
                return Err(PrintError::Cancelled);
            }
            if self.y + self.line_height > self.height - self.margin_y - self.line_height * 2 {
                self.next_page()?;
            }
            let mut x = self.margin_x;
            if self.options.line_numbers {
                let label = if first {
                    format!("{:>6}  ", line.number)
                } else {
                    "        ".into()
                };
                x += self.output(x, self.y, &label, 0x666666)?;
            }
            let remaining = &text[at..];
            let wide: Vec<u16> = remaining.encode_utf16().collect();
            let mut fit = 0;
            let mut size = Default::default();
            if !wide.is_empty()
                && !unsafe {
                    GetTextExtentExPointW(
                        self.dc,
                        PCWSTR(wide.as_ptr()),
                        wide.len() as i32,
                        self.width - self.margin_x - x,
                        Some(&mut fit),
                        None,
                        &mut size,
                    )
                }
                .as_bool()
            {
                return Err(PrintError::Driver("Printer cannot measure text".into()));
            }
            let mut units = 0;
            let mut bytes = 0;
            for c in remaining.chars() {
                if units + c.len_utf16() > fit.max(1) as usize {
                    break;
                }
                units += c.len_utf16();
                bytes += c.len_utf8();
            }
            if bytes == 0 && !remaining.is_empty() {
                bytes = remaining.chars().next().unwrap().len_utf8();
            }
            if bytes < remaining.len() {
                if let Some((boundary, _)) = remaining[..bytes].char_indices().rev().find(|(_, c)| c.is_whitespace()) {
                    if boundary > 0 {
                        bytes = boundary + remaining[boundary..].chars().next().unwrap().len_utf8();
                    }
                }
            }
            let end = at + bytes;
            let mut position = at;
            if self.options.syntax_colors {
                for span in line.spans {
                    let start = span.bytes.start.max(at);
                    let stop = span.bytes.end.min(end);
                    if start < stop {
                        if position < start {
                            x += self.output(x, self.y, &text[position..start], self.options.foreground)?;
                        }
                        x += self.output(x, self.y, &text[start..stop], span.rgb)?;
                        position = stop;
                    }
                }
            }
            if position < end {
                self.output(
                    x,
                    self.y,
                    &text[position..end],
                    if self.options.syntax_colors {
                        self.options.foreground
                    } else {
                        0
                    },
                )?;
            }
            self.y += self.line_height;
            first = false;
            at = end;
            if at >= text.len() {
                break;
            }
        }
        self.summary.lines += 1;
        Ok(())
    }
    fn finish(mut self: Box<Self>, cancel: &AtomicBool) -> Result<PrintSummary, PrintError> {
        if cancel.load(Ordering::Acquire) {
            return Err(PrintError::Cancelled);
        }
        self.end_page()?;
        if unsafe { EndDoc(self.dc) } <= 0 {
            return Err(PrintError::Driver(
                "Printer could not finalize the job; check its queue before retrying".into(),
            ));
        }
        self.finished = true;
        Ok(self.summary.clone())
    }
}
impl Drop for WindowsPrintJob {
    fn drop(&mut self) {
        unsafe {
            if !self.finished {
                AbortDoc(self.dc);
            }
            SelectObject(self.dc, self.old_font);
            let _ = DeleteObject(HGDIOBJ(self.font.0));
            let _ = DeleteDC(self.dc);
        }
    }
}
fn expand_tabs(line: &PrintLine<'_>, width: usize) -> Result<(String, Vec<PrintSpan>), PrintError> {
    let mut output = String::new();
    let mut offsets = vec![0; line.text.len() + 1];
    let mut column = 0;
    for (at, c) in line.text.char_indices() {
        offsets[at] = output.len();
        if c == '\t' {
            let spaces = width - column % width;
            if output.len() + spaces > 256 * 1024 {
                return Err(PrintError::InvalidLine);
            }
            output.extend(std::iter::repeat_n(' ', spaces));
            column += spaces;
        } else {
            if output.len() + c.len_utf8() > 256 * 1024 {
                return Err(PrintError::InvalidLine);
            }
            output.push(c);
            column = if matches!(c, '\r' | '\n') { 0 } else { column + 1 };
        }
    }
    offsets[line.text.len()] = output.len();
    let spans = line
        .spans
        .iter()
        .map(|s| PrintSpan {
            bytes: offsets[s.bytes.start]..offsets[s.bytes.end],
            rgb: s.rgb,
        })
        .collect();
    Ok((output, spans))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tab_expansion_preserves_unicode_style_ranges() {
        let spans = [PrintSpan {
            bytes: 2..6,
            rgb: 0x123456,
        }];
        let line = PrintLine {
            number: 1,
            text: "a\t🙂",
            spans: &spans,
        };
        let (text, expanded) = expand_tabs(&line, 4).unwrap();
        assert_eq!(text, "a   🙂");
        assert_eq!(expanded[0].bytes, 4..8);
    }
    #[test]
    fn invalid_options_fail_before_driver_access() {
        let mut options = PrintOptions::default();
        options.font_size_pt = f64::NAN;
        assert!(matches!(
            WindowsPrintJob::start(
                PrinterSelection {
                    name: vec![],
                    mode: vec![]
                },
                options
            ),
            Err(PrintError::InvalidOptions)
        ));
    }
}
