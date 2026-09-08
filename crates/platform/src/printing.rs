// SPDX-License-Identifier: MPL-2.0
//! OS-neutral, bounded line-at-a-time print contract. Drivers and dialogs stay platform-owned.
use std::{ops::Range, sync::atomic::AtomicBool};
#[derive(Clone, Debug)]
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
    fn default() -> Self { Self { title:"Bareline document".into(),font_family:"Consolas".into(),font_size_pt:10.0,
        margin_mm:12.0,line_numbers:true,header:true,footer:true,syntax_colors:true,foreground:0,background:0xffffff,tab_width:4 } }
}
impl PrintOptions {
    pub fn validate(&self)->Result<(),PrintError> {
        if self.title.len()>4096 || self.font_family.len()>256 || self.title.contains('\0') || self.font_family.contains('\0')
            || !self.font_size_pt.is_finite() || !(6.0..=72.0).contains(&self.font_size_pt)
            || !self.margin_mm.is_finite() || !(0.0..=75.0).contains(&self.margin_mm) || !(1..=16).contains(&self.tab_width) || self.foreground>0xffffff || self.background>0xffffff { return Err(PrintError::InvalidOptions); }
        Ok(())
    }
}
#[derive(Clone,Debug)]
pub struct PrintSpan { pub bytes:Range<usize>, pub rgb:u32 }
pub struct PrintLine<'a> { pub number:usize,pub text:&'a str,pub spans:&'a[PrintSpan] }
#[derive(Clone,Debug,PartialEq,Eq)]
pub enum PrintError { Cancelled,Unavailable(String),Driver(String),InvalidOptions,InvalidLine }
#[derive(Clone,Debug,Default)]
pub struct PrintSummary { pub pages:u32,pub lines:u64 }
pub trait PrintTarget {
    /// Maximum logical line length is 256 KiB. Call only on a bounded print worker.
    fn write_line(&mut self,line:PrintLine<'_>,cancel:&AtomicBool)->Result<(),PrintError>;
    fn finish(self:Box<Self>,cancel:&AtomicBool)->Result<PrintSummary,PrintError>;
}
