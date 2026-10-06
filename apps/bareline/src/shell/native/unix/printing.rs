// SPDX-License-Identifier: MPL-2.0
//! Printing is a later tier on these systems (ADR-C tier 3).
use bareline_platform::{
    Capability, Unsupported,
    printing::{PrintError, PrintLine, PrintOptions, PrintSummary, PrintTarget},
};
use std::sync::atomic::AtomicBool;

fn unavailable() -> PrintError {
    PrintError::Unavailable(
        Unsupported {
            capability: Capability::Printing,
        }
        .to_string(),
    )
}
pub struct PrinterSelection;
pub fn choose_printer(_owner: Option<super::RawWindow>) -> Result<Option<PrinterSelection>, PrintError> {
    Err(unavailable())
}
pub struct PrintJob;
impl PrintJob {
    pub fn start(_selection: PrinterSelection, _options: PrintOptions) -> Result<Self, PrintError> {
        Err(unavailable())
    }
}
impl PrintTarget for PrintJob {
    fn write_line(&mut self, _line: PrintLine<'_>, _cancel: &AtomicBool) -> Result<(), PrintError> {
        Err(unavailable())
    }
    fn finish(self: Box<Self>, _cancel: &AtomicBool) -> Result<PrintSummary, PrintError> {
        Err(unavailable())
    }
}
