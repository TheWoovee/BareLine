// SPDX-License-Identifier: MPL-2.0
//! PR-007 command projection. Codec state and transactions remain workspace-owned.
use bareline_commands::{Action, CommandContext, CommandId, CommandRegistry, CommandSpec, CommandState};
use bareline_file_io::codecs::{Encoding, state::{EncodingState, Eol}};

pub struct CodecChoice {
    pub encoding: Encoding,
    pub label: &'static str,
    pub interpret: &'static str,
    pub convert: &'static str,
}
macro_rules! choices {
    ($(($variant:ident, $label:literal, $key:literal)),* $(,)?) => {
        pub const CODECS: &[CodecChoice] = &[$(CodecChoice {
            encoding: Encoding::$variant, label: $label,
            interpret: concat!("encoding.interpret.", $key),
            convert: concat!("encoding.convert.", $key),
        }),*];
    };
}
choices! {
    (Utf8, "UTF-8", "utf8"), (Utf16Le, "UTF-16 LE", "utf16le"),
    (Utf16Be, "UTF-16 BE", "utf16be"), (Utf32Le, "UTF-32 LE", "utf32le"),
    (Utf32Be, "UTF-32 BE", "utf32be"), (Latin1, "ISO-8859-1", "latin1"),
    (Windows1250, "Windows-1250", "windows1250"), (Windows1251, "Windows-1251", "windows1251"),
    (Windows1252, "Windows-1252", "windows1252"), (Windows1253, "Windows-1253", "windows1253"),
    (Windows1254, "Windows-1254", "windows1254"), (Windows1255, "Windows-1255", "windows1255"),
    (Windows1256, "Windows-1256", "windows1256"), (Windows1257, "Windows-1257", "windows1257"),
    (Windows1258, "Windows-1258", "windows1258"), (ShiftJis, "Shift-JIS", "shiftjis"),
    (Gbk, "GBK", "gbk"), (Big5, "Big5", "big5"),
    (EucJp, "EUC-JP", "eucjp"), (EucKr, "EUC-KR", "euckr"),
}
pub const ROOT: &[&str] = &["encoding.info", "encoding.choose_interpret", "encoding.choose_convert", "encoding.bom_on", "encoding.bom_off", "encoding.eol", "encoding.binary"];
pub const EOLS: &[&str] = &["encoding.eol.lf", "encoding.eol.crlf", "encoding.eol.cr", "encoding.eol.selection_lf", "encoding.eol.selection_crlf", "encoding.eol.selection_cr"];
pub const BINARY: &[&str] = &["encoding.binary.info", "encoding.binary.readonly", "encoding.binary.edit"];

pub fn register(registry: &mut CommandRegistry) {
    for (id, title) in [
        ("encoding.choose", "Encoding…"), ("encoding.info", "Encoding Details"),
        ("encoding.choose_interpret", "Interpret Original Bytes As…"),
        ("encoding.choose_convert", "Convert Save Encoding To…"),
        ("encoding.bom_on", "Write Byte Order Mark"), ("encoding.bom_off", "Omit Byte Order Mark"),
        ("encoding.eol", "Line Endings…"), ("encoding.eol.lf", "Convert Document to LF"),
        ("encoding.eol.crlf", "Convert Document to CRLF"), ("encoding.eol.cr", "Convert Document to CR"),
        ("encoding.eol.selection_lf", "Convert Selection to LF"),
        ("encoding.eol.selection_crlf", "Convert Selection to CRLF"),
        ("encoding.eol.selection_cr", "Convert Selection to CR"),
        ("encoding.binary", "Binary Warning Options…"),
        ("encoding.binary.info", "Binary-like bytes detected; choose how to open text"),
        ("encoding.binary.readonly", "Keep Read-only Text"), ("encoding.binary.edit", "Allow Text Editing"),
    ] {
        register_one(registry, id, title);
    }
    for codec in CODECS {
        // Titles are stable and the context supplies operation-specific labels.
        register_one(registry, codec.interpret, codec.label);
        register_one(registry, codec.convert, codec.label);
    }
}
fn register_one(registry: &mut CommandRegistry, id: &'static str, title: &'static str) {
    registry.register(CommandSpec { id: CommandId(id), title, category: "Encoding", shortcut: "", action: Action::Contributed(CommandId(id)) }).expect("unique encoding command");
    let menu_path = if id.starts_with("encoding.interpret.") { "Encoding > Interpret As" }
        else if id.starts_with("encoding.convert.") { "Encoding > Convert To" }
        else if id.starts_with("encoding.eol.") { "Encoding > Line Endings" }
        else if id.starts_with("encoding.binary.") { "Encoding > Binary Warning" }
        else { "Encoding" };
    registry.set_presentation(CommandId(id), bareline_commands::CommandPresentation {
        menu_path: menu_path.into(), keywords: vec!["codec".into(), "Unicode".into()], ..Default::default()
    }).expect("registered encoding command");
}
pub fn label(encoding: Encoding) -> &'static str {
    CODECS.iter().find(|item| item.encoding == encoding).expect("complete codec catalog").label
}
pub fn summary(state: &EncodingState) -> String {
    format!("{} → {}; {:?}; BOM {}; {} invalid spans / {} original bytes",
        label(state.interpreted()), label(state.save_target), state.confidence,
        if state.bom { "on" } else { "off" }, state.invalid_span_count, state.invalid_byte_count)
}
pub fn annotate(context: &mut CommandContext, state: Option<&EncodingState>, busy: bool, read_only: bool) {
    let unavailable = state.is_none() || busy;
    for id in ROOT.iter().chain(EOLS).chain(BINARY).copied().chain(["encoding.choose"])
        .chain(CODECS.iter().flat_map(|choice| [choice.interpret, choice.convert])) {
        if unavailable { context.states.insert(CommandId(id), CommandState::disabled("Wait for an open document to finish loading")); }
    }
    let Some(state) = state else { return; };
    let info = CommandState { label: Some(summary(state)), ..CommandState::disabled("Encoding detection and invalid-byte information") };
    context.states.insert(CommandId("encoding.info"), info);
    context.states.insert(CommandId("encoding.binary.info"), CommandState::disabled("Binary-like bytes detected; original bytes are preserved"));
    for choice in CODECS {
        for (id, operation, checked) in [(choice.interpret, "Interpret as", state.interpreted() == choice.encoding), (choice.convert, "Convert to", state.save_target == choice.encoding)] {
            let mut item = if unavailable || (read_only && id == choice.convert) { CommandState::disabled("Document is busy or read-only") } else { CommandState::default() };
            item.label = Some(format!("{operation} {}", choice.label)); item.checked = checked;
            context.states.insert(CommandId(id), item);
        }
    }
    for (id, checked) in [("encoding.bom_on", state.bom), ("encoding.bom_off", !state.bom)] {
        let mut item = if unavailable || read_only || (id == "encoding.bom_on" && state.save_target.bom().is_empty()) { CommandState::disabled("BOM is unavailable for this target or document") } else { CommandState::default() };
        item.checked = checked; context.states.insert(CommandId(id), item);
    }
    if read_only { for id in EOLS { context.states.insert(CommandId(id), CommandState::disabled("Document is read-only")); } }
    if !state.binary_warning { context.states.insert(CommandId("encoding.binary"), CommandState::disabled("No binary warning for this document")); }
}
pub fn eol_action(id: &str) -> Option<(Eol, bool)> {
    Some(match id {
        "encoding.eol.lf" => (Eol::Lf, false), "encoding.eol.crlf" => (Eol::CrLf, false), "encoding.eol.cr" => (Eol::Cr, false),
        "encoding.eol.selection_lf" => (Eol::Lf, true), "encoding.eol.selection_crlf" => (Eol::CrLf, true), "encoding.eol.selection_cr" => (Eol::Cr, true), _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_file_io::codecs::{Confidence, Detection};
    #[test]
    fn readonly_and_unsupported_bom_cannot_dispatch_but_binary_choice_can() {
        let mut registry = CommandRegistry::default();
        register(&mut registry);
        let state = EncodingState::new(Detection { encoding: Encoding::Latin1, confidence: Confidence::LegacySample, bom: false, binary_warning: true });
        let mut context = CommandContext::default();
        annotate(&mut context, Some(&state), false, true);
        assert!(registry.dispatch_in(CommandId("encoding.convert.utf8"), &context).is_err());
        assert!(registry.dispatch_in(CommandId("encoding.bom_on"), &context).is_err());
        assert!(registry.dispatch_in(CommandId("encoding.eol.lf"), &context).is_err());
        assert!(registry.dispatch_in(CommandId("encoding.binary.edit"), &context).is_ok());
        assert!(registry.dispatch_in(CommandId("encoding.interpret.utf8"), &context).is_ok());
        let mut writable = CommandContext::default();
        annotate(&mut writable, Some(&state), false, false);
        assert!(registry.dispatch_in(CommandId("encoding.bom_on"), &writable).is_err());
        assert!(registry.dispatch_in(CommandId("encoding.convert.utf8"), &writable).is_ok());
    }
}
