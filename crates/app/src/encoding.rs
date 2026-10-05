// SPDX-License-Identifier: MPL-2.0
//! PR-007 command projection. Codec state and transactions remain workspace-owned.
use bareline_commands::{Action, CommandContext, CommandId, CommandRegistry, CommandSpec, CommandState, MenuTemplate};
use bareline_file_io::codecs::{
    Encoding,
    state::{EncodingState, Eol},
};

pub struct CodecChoice {
    pub encoding: Encoding,
    pub label: &'static str,
    pub interpret: &'static str,
    pub convert: &'static str,
    pub family: CodecFamily,
}
/// Groups the character sets the way a person looks for them, so the picker can
/// be scanned by writing system rather than by an opaque list of code pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CodecFamily {
    Unicode,
    Western,
    CentralEuropean,
    Cyrillic,
    Greek,
    Turkish,
    Baltic,
    Arabic,
    Hebrew,
    Vietnamese,
    Thai,
    EastAsian,
    DosOem,
    Mac,
}
impl CodecFamily {
    pub const fn label(self) -> &'static str {
        match self {
            CodecFamily::Unicode => "Unicode",
            CodecFamily::Western => "Western European",
            CodecFamily::CentralEuropean => "Central European",
            CodecFamily::Cyrillic => "Cyrillic",
            CodecFamily::Greek => "Greek",
            CodecFamily::Turkish => "Turkish",
            CodecFamily::Baltic => "Baltic",
            CodecFamily::Arabic => "Arabic",
            CodecFamily::Hebrew => "Hebrew",
            CodecFamily::Vietnamese => "Vietnamese",
            CodecFamily::EastAsian => "East Asian",
            CodecFamily::Thai => "Thai",
            CodecFamily::DosOem => "DOS/OEM",
            CodecFamily::Mac => "Mac",
        }
    }
    /// Display order for the picker and the Encoding submenus: Unicode first,
    /// then legacy families.
    pub const ORDER: [CodecFamily; 14] = [
        CodecFamily::Unicode,
        CodecFamily::Western,
        CodecFamily::CentralEuropean,
        CodecFamily::Cyrillic,
        CodecFamily::Greek,
        CodecFamily::Turkish,
        CodecFamily::Baltic,
        CodecFamily::Arabic,
        CodecFamily::Hebrew,
        CodecFamily::Vietnamese,
        CodecFamily::Thai,
        CodecFamily::EastAsian,
        CodecFamily::DosOem,
        CodecFamily::Mac,
    ];
}
// Labels come from the one canonical table, `Encoding::display_name` (UI-07).
// Families are listed in `CodecFamily::ORDER`. The leading (Unicode) family sits
// directly in Interpret As and Convert To; every later family gets a submenu.
macro_rules! choices {
    (@choice $family:ident, $variant:ident, $key:literal) => {
        CodecChoice {
            encoding: Encoding::$variant, label: Encoding::$variant.display_name(),
            interpret: concat!("encoding.interpret.", $key),
            convert: concat!("encoding.convert.", $key),
            family: CodecFamily::$family,
        }
    };
    (@menu $op:literal, [$($lead:literal),*], $([$family:ident: $($key:literal),*])*) => {
        &[
            $(MenuTemplate::Command(concat!($op, $lead)),)*
            MenuTemplate::Separator,
            $(MenuTemplate::Submenu(
                CodecFamily::$family.label(),
                &[$(MenuTemplate::Command(concat!($op, $key))),*],
            ),)*
        ]
    };
    ($lead:ident: [$(($lv:ident, $lk:literal)),* $(,)?],
     $($family:ident: [$(($variant:ident, $key:literal)),* $(,)?]),* $(,)?) => {
        pub const CODECS: &[CodecChoice] = &[
            $(choices!(@choice $lead, $lv, $lk),)*
            $($(choices!(@choice $family, $variant, $key),)*)*
        ];
        /// Encoding ▸ Interpret As, grouped by family (BIZ-09).
        pub const INTERPRET_MENU: &[MenuTemplate] =
            choices!(@menu "encoding.interpret.", [$($lk),*], $([$family: $($key),*])*);
        /// Encoding ▸ Convert To (the save encoding), grouped like Interpret As.
        pub const CONVERT_MENU: &[MenuTemplate] =
            choices!(@menu "encoding.convert.", [$($lk),*], $([$family: $($key),*])*);
    };
}
choices! {
    Unicode: [
        (Utf8, "utf8"), (Utf16Le, "utf16le"), (Utf16Be, "utf16be"),
        (Utf32Le, "utf32le"), (Utf32Be, "utf32be"),
    ],
    Western: [
        (Latin1, "latin1"), (Windows1252, "windows1252"), (Iso8859_10, "iso8859_10"),
        (Iso8859_14, "iso8859_14"), (Iso8859_15, "iso8859_15"),
    ],
    CentralEuropean: [
        (Windows1250, "windows1250"), (Iso8859_2, "iso8859_2"), (Iso8859_16, "iso8859_16"),
    ],
    Cyrillic: [
        (Windows1251, "windows1251"), (Iso8859_5, "iso8859_5"), (Koi8R, "koi8r"), (Koi8U, "koi8u"),
    ],
    Greek: [(Windows1253, "windows1253"), (Iso8859_7, "iso8859_7")],
    Turkish: [(Windows1254, "windows1254"), (Iso8859_3, "iso8859_3")],
    Baltic: [(Windows1257, "windows1257"), (Iso8859_4, "iso8859_4"), (Iso8859_13, "iso8859_13")],
    Arabic: [(Windows1256, "windows1256"), (Iso8859_6, "iso8859_6")],
    Hebrew: [(Windows1255, "windows1255"), (Iso8859_8, "iso8859_8")],
    Vietnamese: [(Windows1258, "windows1258")],
    Thai: [(Windows874, "windows874")],
    EastAsian: [
        (ShiftJis, "shiftjis"), (Gbk, "gbk"), (Big5, "big5"),
        (EucJp, "eucjp"), (EucKr, "euckr"),
    ],
    DosOem: [(Cp437, "cp437"), (Cp850, "cp850"), (Cp852, "cp852"), (Cp866, "cp866")],
    Mac: [(MacRoman, "macroman"), (MacCyrillic, "maccyrillic")],
}
/// The character sets grouped by family, in display order, for the searchable
/// "Character Sets…" picker. Empty families are omitted.
pub fn character_sets() -> Vec<(CodecFamily, Vec<&'static CodecChoice>)> {
    CodecFamily::ORDER
        .into_iter()
        .filter_map(|family| {
            let members: Vec<_> = CODECS.iter().filter(|codec| codec.family == family).collect();
            (!members.is_empty()).then_some((family, members))
        })
        .collect()
}
/// Case-insensitive search of the character sets by label or family name, for
/// the picker's filter box. An empty query returns everything in family order.
pub fn charset_matches(query: &str) -> Vec<&'static CodecChoice> {
    let needle = query.trim().to_lowercase();
    let mut out = Vec::new();
    for (family, members) in character_sets() {
        for codec in members {
            if needle.is_empty()
                || codec.label.to_lowercase().contains(&needle)
                || family.label().to_lowercase().contains(&needle)
            {
                out.push(codec);
            }
        }
    }
    out
}
pub const ROOT: &[&str] = &[
    "encoding.info",
    "encoding.failure",
    "encoding.choose_interpret",
    "encoding.choose_convert",
    "encoding.bom_on",
    "encoding.bom_off",
    "encoding.eol",
    "encoding.binary",
];
pub const EOLS: &[&str] = &[
    "encoding.eol.lf",
    "encoding.eol.crlf",
    "encoding.eol.cr",
    "encoding.eol.selection_lf",
    "encoding.eol.selection_crlf",
    "encoding.eol.selection_cr",
];
pub const BINARY: &[&str] = &[
    "encoding.binary.info",
    "encoding.binary.readonly",
    "encoding.binary.edit",
];
/// Actions of the per-document binary notice (UI-01). "Close" records the
/// read-only decision so the notice does not return for this document.
pub const BINARY_NOTICE_ACTIONS: [(&str, &str); 2] = [
    ("Edit as text", "encoding.binary.edit"),
    ("Close", "encoding.binary.readonly"),
];
/// Band a view reserves above its text while the binary notice is pending, so
/// the notice never covers the document's first lines.
pub const BINARY_NOTICE_HEIGHT: f32 = 40.0;
/// Non-modal notice shown inside the document view instead of a blocking prompt.
pub fn binary_notice(name: &str) -> String {
    format!("{name} contains binary-like bytes. It is open read-only.")
}

pub fn register(registry: &mut CommandRegistry) {
    for (id, title) in [
        ("encoding.choose", "Encoding…"),
        ("encoding.info", "Encoding Details"),
        ("encoding.charsets", "Character Sets…"),
        ("encoding.failure", "Show Encoding Save Failure"),
        ("encoding.choose_interpret", "Interpret Original Bytes As…"),
        ("encoding.choose_convert", "Convert Save Encoding To…"),
        ("encoding.bom_on", "Write Byte Order Mark"),
        ("encoding.bom_off", "Omit Byte Order Mark"),
        ("encoding.eol", "Line Endings…"),
        ("encoding.eol.lf", "Convert Document to LF"),
        ("encoding.eol.crlf", "Convert Document to CRLF"),
        ("encoding.eol.cr", "Convert Document to CR"),
        ("encoding.eol.selection_lf", "Convert Selection to LF"),
        ("encoding.eol.selection_crlf", "Convert Selection to CRLF"),
        ("encoding.eol.selection_cr", "Convert Selection to CR"),
        ("encoding.binary", "Binary Warning Options…"),
        (
            "encoding.binary.info",
            "Binary-like bytes detected; choose how to open text",
        ),
        ("encoding.binary.readonly", "Keep Read-only Text"),
        ("encoding.binary.edit", "Allow Text Editing"),
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
    registry
        .register(CommandSpec {
            id: CommandId(id),
            title,
            category: "Encoding",
            shortcut: "",
            action: Action::Contributed(CommandId(id)),
        })
        .expect("unique encoding command");
    let menu_path = if id.starts_with("encoding.interpret.") {
        "Encoding > Interpret As"
    } else if id.starts_with("encoding.convert.") {
        "Encoding > Convert To"
    } else if id.starts_with("encoding.eol.") {
        "Encoding > Line Endings"
    } else if id.starts_with("encoding.binary.") {
        "Encoding > Binary Warning"
    } else {
        "Encoding"
    };
    // The summary line is a diagnostic string, not a menu command (it fronts the
    // status-bar click), and the "…" chooser popups are superseded by the curated
    // Encoding menu's own submenus. Keep them dispatchable but out of the menus.
    let internal = matches!(
        id,
        "encoding.info" | "encoding.choose" | "encoding.choose_interpret" | "encoding.choose_convert" | "encoding.eol"
    );
    // Codec commands name the family submenu `INTERPRET_MENU`/`CONVERT_MENU` give them.
    let family = CODECS
        .iter()
        .find(|codec| codec.interpret == id || codec.convert == id)
        .filter(|codec| codec.family != CodecFamily::Unicode)
        .map(|codec| format!(" > {}", codec.family.label()))
        .unwrap_or_default();
    registry
        .set_presentation(
            CommandId(id),
            bareline_commands::CommandPresentation {
                menu_path: format!("{menu_path}{family}"),
                keywords: vec!["codec".into(), "Unicode".into()],
                internal,
                ..Default::default()
            },
        )
        .expect("registered encoding command");
}
pub fn label(encoding: Encoding) -> &'static str {
    CODECS
        .iter()
        .find(|item| item.encoding == encoding)
        .expect("complete codec catalog")
        .label
}
/// How the encoding was chosen, in the user's words (UI-03).
pub(crate) fn confidence_label(confidence: bareline_file_io::codecs::Confidence) -> &'static str {
    use bareline_file_io::codecs::Confidence;
    match confidence {
        Confidence::Bom => "detected from the byte order mark",
        Confidence::Utf8Sample => "detected as valid UTF-8",
        Confidence::Utf16Sample => "detected from its UTF-16 byte pattern",
        Confidence::LegacySample => "detected from a sample",
        Confidence::LegacyFallback => "guessed",
    }
}
pub fn summary(state: &EncodingState) -> String {
    let mut summary = format!(
        "{} → {}; {}; BOM {}; {} invalid spans / {} original bytes",
        label(state.interpreted()),
        label(state.save_target),
        confidence_label(state.confidence),
        if state.bom { "on" } else { "off" },
        state.invalid_span_count,
        state.invalid_byte_count
    );
    let candidates: Vec<_> = state.uncertain_candidates().map(label).collect();
    if !candidates.is_empty() {
        summary.push_str(&format!("; may be wrong, likely {}", candidates.join(" or ")));
    }
    summary
}
/// "Encoding may be wrong" hint (FIO-05): an ambiguous legacy sample fell back to
/// the default encoding. Names the likeliest alternatives and where to pick them.
pub fn detection_hint(state: &EncodingState) -> Option<String> {
    let candidates: Vec<_> = state.uncertain_candidates().map(label).collect();
    (!candidates.is_empty()).then(|| {
        format!(
            "Encoding may be wrong: shown as {}. Likely {}; choose Encoding > Interpret As.",
            label(state.interpreted()),
            candidates.join(" or ")
        )
    })
}
/// Coordinates are UTF-8 text bytes in the captured revision, never raw-file offsets.
pub fn failure_description(
    revision: u64,
    range: std::ops::Range<bareline_document::TextOffset>,
    reason: &str,
) -> String {
    format!(
        "Save refused: {reason}; text bytes {}–{} (end exclusive), revision {revision}",
        range.start.0, range.end.0
    )
}
pub fn annotate(context: &mut CommandContext, state: Option<&EncodingState>, busy: bool, read_only: bool) {
    let unavailable = state.is_none() || busy;
    for id in ROOT
        .iter()
        .chain(EOLS)
        .chain(BINARY)
        .copied()
        .chain(["encoding.choose", "encoding.charsets"])
        .chain(CODECS.iter().flat_map(|choice| [choice.interpret, choice.convert]))
    {
        if unavailable {
            context.states.insert(
                CommandId(id),
                CommandState::disabled("Wait for an open document to finish loading"),
            );
        }
    }
    let Some(state) = state else {
        return;
    };
    let info = CommandState {
        label: Some(summary(state)),
        ..CommandState::disabled("Encoding detection and invalid-byte information")
    };
    context.states.insert(CommandId("encoding.info"), info);
    context.states.insert(
        CommandId("encoding.binary.info"),
        CommandState::disabled("Binary-like bytes detected; original bytes are preserved"),
    );
    // Native/workspace composition enables this only for an actual captured failure.
    context.states.insert(
        CommandId("encoding.failure"),
        CommandState::disabled("No encoding save failure for this document"),
    );
    for choice in CODECS {
        for (id, operation, checked) in [
            (choice.interpret, "Interpret as", state.interpreted() == choice.encoding),
            (choice.convert, "Convert to", state.save_target == choice.encoding),
        ] {
            let mut item = if unavailable || (read_only && id == choice.convert) {
                CommandState::disabled("Document is busy or read-only")
            } else {
                CommandState::default()
            };
            item.label = Some(format!("{operation} {}", choice.label));
            item.checked = checked;
            context.states.insert(CommandId(id), item);
        }
    }
    for (id, checked) in [("encoding.bom_on", state.bom), ("encoding.bom_off", !state.bom)] {
        let mut item = if unavailable || read_only || (id == "encoding.bom_on" && state.save_target.bom().is_empty()) {
            CommandState::disabled("BOM is unavailable for this target or document")
        } else {
            CommandState::default()
        };
        item.checked = checked;
        context.states.insert(CommandId(id), item);
    }
    if read_only {
        for id in EOLS {
            context
                .states
                .insert(CommandId(id), CommandState::disabled("Document is read-only"));
        }
    }
    if !state.binary_warning {
        context.states.insert(
            CommandId("encoding.binary"),
            CommandState::disabled("No binary warning for this document"),
        );
    }
}
pub fn eol_action(id: &str) -> Option<(Eol, bool)> {
    Some(match id {
        "encoding.eol.lf" => (Eol::Lf, false),
        "encoding.eol.crlf" => (Eol::CrLf, false),
        "encoding.eol.cr" => (Eol::Cr, false),
        "encoding.eol.selection_lf" => (Eol::Lf, true),
        "encoding.eol.selection_crlf" => (Eol::CrLf, true),
        "encoding.eol.selection_cr" => (Eol::Cr, true),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_file_io::codecs::{Confidence, Detection};
    /// UI-07: pickers, menus and the status bar share one label per encoding,
    /// never the `Debug` spelling ("ShiftJis").
    #[test]
    fn every_codec_label_is_the_canonical_display_name() {
        for codec in CODECS {
            assert_eq!(codec.label, codec.encoding.display_name());
            assert_eq!(label(codec.encoding), codec.encoding.status_label(false));
            assert_ne!(codec.label, format!("{:?}", codec.encoding), "{:?}", codec.encoding);
        }
        assert_eq!(Encoding::ShiftJis.status_label(false), "Shift-JIS (Japanese)");
        assert_eq!(Encoding::Utf16Le.status_label(true), "UTF-16 LE BOM");
    }
    /// BIZ-09: Interpret As and Convert To (the save encoding) list every catalog
    /// entry exactly once, grouped by family in display order.
    #[test]
    fn encoding_menus_group_every_catalog_entry_exactly_once() {
        use bareline_commands::MenuItem;
        for encoding in Encoding::ALL {
            let count = CODECS.iter().filter(|codec| codec.encoding == *encoding).count();
            assert_eq!(count, 1, "{encoding:?}");
        }
        assert_eq!(CODECS.len(), Encoding::ALL.len());
        let mut registry = CommandRegistry::default();
        register(&mut registry);
        let model = crate::menus::curated_model(&registry);
        let encoding_menu = model
            .items
            .iter()
            .find_map(|item| match item {
                MenuItem::Submenu { title, items } if title == "Encoding" => Some(items),
                _ => None,
            })
            .expect("Encoding menu present");
        for (title, interpret) in [("Interpret As", true), ("Convert To", false)] {
            let items = encoding_menu
                .iter()
                .find_map(|item| match item {
                    MenuItem::Submenu { title: name, items } if name == title => Some(items),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{title} submenu present"));
            // Unicode entries lead the submenu; each later family is one submenu.
            let (mut families, mut seen) = (vec![CodecFamily::Unicode], Vec::new());
            for item in items {
                match item {
                    MenuItem::Command(id) => seen.push((CodecFamily::Unicode, id.0)),
                    MenuItem::Separator => {}
                    MenuItem::Submenu { title: name, items } => {
                        let family = CodecFamily::ORDER
                            .into_iter()
                            .find(|family| family.label() == name.as_str())
                            .unwrap_or_else(|| panic!("{name} is a codec family"));
                        families.push(family);
                        for item in items {
                            let MenuItem::Command(id) = item else {
                                panic!("{name} holds only codec commands");
                            };
                            seen.push((family, id.0));
                        }
                    }
                }
            }
            assert_eq!(families, CodecFamily::ORDER, "{title}: family order");
            let expected: Vec<_> = CODECS
                .iter()
                .map(|codec| (codec.family, if interpret { codec.interpret } else { codec.convert }))
                .collect();
            assert_eq!(seen, expected, "{title}");
        }
        let path = |id| registry.presentation(CommandId(id)).unwrap().menu_path.clone();
        assert_eq!(path("encoding.interpret.koi8r"), "Encoding > Interpret As > Cyrillic");
        assert_eq!(path("encoding.convert.cp437"), "Encoding > Convert To > DOS/OEM");
        assert_eq!(path("encoding.convert.utf8"), "Encoding > Convert To");
    }
    #[test]
    fn character_sets_cover_every_codec_and_are_searchable() {
        let grouped = character_sets();
        let counted: usize = grouped.iter().map(|(_, members)| members.len()).sum();
        assert_eq!(counted, CODECS.len(), "every codec is grouped exactly once");
        assert_eq!(grouped[0].0, CodecFamily::Unicode, "Unicode leads the picker");
        // ANSI / Windows-1252 is selectable and reachable by family or by number.
        assert!(
            charset_matches("western")
                .iter()
                .any(|codec| codec.encoding == Encoding::Windows1252)
        );
        assert!(
            charset_matches("1252")
                .iter()
                .any(|codec| codec.encoding == Encoding::Windows1252)
        );
        assert_eq!(charset_matches("").len(), CODECS.len());
        // The debug summary is internal so it never appears in a menu; the new
        // picker command is registered.
        let mut registry = CommandRegistry::default();
        register(&mut registry);
        assert!(registry.presentation(CommandId("encoding.info")).unwrap().internal);
        assert!(registry.presentation(CommandId("encoding.charsets")).is_some());
    }
    #[test]
    fn ambiguous_detection_names_likely_encodings_until_interpreted() {
        let mut state = EncodingState::new(Detection {
            encoding: Encoding::Windows1252,
            confidence: Confidence::LegacyFallback,
            bom: false,
            binary_warning: false,
            candidates: [Some(Encoding::Gbk), Some(Encoding::Big5), None],
        });
        let hint = detection_hint(&state).expect("ambiguous detection offers a hint");
        assert!(hint.contains("Windows-1252 (Western / ANSI)"), "{hint}");
        assert!(
            hint.contains("GBK (Simplified Chinese) or Big5 (Traditional Chinese)"),
            "{hint}"
        );
        assert!(summary(&state).contains("may be wrong"));
        state.user_override = Some(Encoding::Gbk);
        assert_eq!(detection_hint(&state), None);
        assert!(!summary(&state).contains("may be wrong"));
        let confident = EncodingState::new(bareline_file_io::codecs::detect(b"plain"));
        assert_eq!(detection_hint(&confident), None);
    }
    #[test]
    fn readonly_and_unsupported_bom_cannot_dispatch_but_binary_choice_can() {
        let mut registry = CommandRegistry::default();
        register(&mut registry);
        let state = EncodingState::new(Detection {
            encoding: Encoding::Latin1,
            confidence: Confidence::LegacySample,
            bom: false,
            binary_warning: true,
            candidates: [None; 3],
        });
        let mut context = CommandContext::default();
        annotate(&mut context, Some(&state), false, true);
        assert!(
            registry
                .dispatch_in(CommandId("encoding.convert.utf8"), &context)
                .is_err()
        );
        assert!(registry.dispatch_in(CommandId("encoding.bom_on"), &context).is_err());
        assert!(registry.dispatch_in(CommandId("encoding.eol.lf"), &context).is_err());
        assert!(
            registry
                .dispatch_in(CommandId("encoding.binary.edit"), &context)
                .is_ok()
        );
        assert!(
            registry
                .dispatch_in(CommandId("encoding.interpret.utf8"), &context)
                .is_ok()
        );
        let mut writable = CommandContext::default();
        annotate(&mut writable, Some(&state), false, false);
        assert!(registry.dispatch_in(CommandId("encoding.bom_on"), &writable).is_err());
        assert!(
            registry
                .dispatch_in(CommandId("encoding.convert.utf8"), &writable)
                .is_ok()
        );
    }
}
