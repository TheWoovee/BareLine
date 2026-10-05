// SPDX-License-Identifier: MPL-2.0
//! Bounded data-only migration. No files are followed and no plugins/macros execute.
use std::collections::BTreeMap;
#[derive(Clone, Debug)]
pub enum Preference {
    Bool(bool),
    Integer(i64),
    Text(String),
    Colors(BTreeMap<String, String>),
}
#[derive(Clone, Debug, Default)]
pub struct ImportReport {
    pub preferences: BTreeMap<String, Preference>,
    pub shortcuts: Vec<(&'static str, String)>,
    pub paths: Vec<String>,
    pub user_language: bool,
    pub notes: Vec<String>,
}
impl ImportReport {
    pub fn render(&self) -> String {
        let mut text = String::from(
            "Notepad++ Import Review\n\nReview maps below. Apply Reviewed Import submits preferences/shortcuts/UDL through normal validation; submission results appear in the report notes. File paths remain an explicit separate open action.\n\n",
        );
        for (key, value) in &self.preferences {
            text.push_str(&format!("Map {key}: {value:?}\n"));
        }
        for (id, key) in &self.shortcuts {
            text.push_str(&format!("Shortcut {id}: {key}\n"));
        }
        for path in &self.paths {
            text.push_str(&format!("Local file candidate: {path}\n"));
        }
        for note in &self.notes {
            text.push_str(&format!("{note}\n"));
        }
        text
    }
}
pub fn parse(bytes: &[u8]) -> Result<ImportReport, String> {
    use quick_xml::{Reader, events::Event};
    if bytes.len() > 1024 * 1024 {
        return Err("Import exceeds 1 MiB".into());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "Import must be UTF-8 XML")?;
    let mut reader = Reader::from_str(text);
    reader.config_mut().check_end_names = true;
    let mut xml_version = quick_xml::XmlVersion::Implicit1_0;
    let mut report = ImportReport::default();
    let mut depth = 0usize;
    let mut count = 0usize;
    let mut roots = 0usize;
    loop {
        let next = reader.read_event().map_err(|e| e.to_string())?;
        let is_start = matches!(&next, Event::Start(_));
        match next {
            Event::Decl(ref declaration) => {
                if roots != 0 {
                    return Err("XML declaration must precede the root".into());
                }
                xml_version = match declaration.version().map_err(|e| e.to_string())?.as_ref() {
                    b"1.0" => quick_xml::XmlVersion::Explicit1_0,
                    b"1.1" => quick_xml::XmlVersion::Explicit1_1,
                    _ => return Err("Unsupported XML version".into()),
                };
            }
            Event::DocType(_) | Event::GeneralRef(_) => return Err("DTD and general entities are not imported".into()),
            Event::Start(ref event) | Event::Empty(ref event) => {
                count += 1;
                if count > 16384 {
                    return Err("Import element limit".into());
                }
                if depth == 0 {
                    roots += 1;
                    if roots > 1 {
                        return Err("Multiple XML roots".into());
                    }
                }
                let mut attrs = BTreeMap::new();
                for attr in event.attributes() {
                    let attr = attr.map_err(|e| e.to_string())?;
                    let key = std::str::from_utf8(attr.key.as_ref())
                        .map_err(|_| "Invalid attribute")?
                        .to_owned();
                    let value = attr
                        .decoded_and_normalized_value(xml_version, reader.decoder())
                        .map_err(|e| e.to_string())?
                        .into_owned();
                    if value.len() > 4096 {
                        return Err("Attribute limit".into());
                    }
                    attrs.insert(key, value);
                }
                let name = event.name();
                let tag = std::str::from_utf8(name.as_ref()).map_err(|_| "Invalid element")?;
                let get = |key: &str| attrs.get(key).map(String::as_str).unwrap_or("");
                match tag {
                    "UserLang" => {
                        report.user_language = true;
                        report.notes.push("User-defined language uses Bareline's validated UDL mapper; unsupported constructs are reported there.".into());
                    }
                    "GUIConfig" => match get("name") {
                        "TabSetting" => {
                            if let Ok(size) = get("size").parse::<i64>() {
                                if (1..=16).contains(&size) {
                                    report
                                        .preferences
                                        .insert("editor.tab.width".into(), Preference::Integer(size));
                                }
                            }
                            if matches!(get("replaceBySpace"), "yes" | "no") {
                                report.preferences.insert(
                                    "editor.insert_spaces".into(),
                                    Preference::Bool(get("replaceBySpace") == "yes"),
                                );
                            }
                        }
                        "ScintillaPrimaryView" => {
                            for (attribute, key) in [
                                ("lineNumberMargin", "editor.line_numbers"),
                                ("currentLineHilightShow", "editor.currentLine.highlight"),
                            ] {
                                if matches!(get(attribute), "show" | "hide") {
                                    report
                                        .preferences
                                        .insert(key.into(), Preference::Bool(get(attribute) == "show"));
                                }
                            }
                            if matches!(get("wrap"), "yes" | "no") {
                                report.preferences.insert(
                                    "editor.wrap.mode".into(),
                                    Preference::Text(if get("wrap") == "yes" { "viewport" } else { "off" }.into()),
                                );
                            }
                        }
                        other => {
                            if report.notes.len() < 256 {
                                report
                                    .notes
                                    .push(format!("Skipped preference {other}: no safe direct mapping"));
                            }
                        }
                    },
                    "WidgetStyle" => {
                        if get("name") == "Default Style" {
                            let mut colors = BTreeMap::new();
                            for (attr, token) in [("fgColor", "text"), ("bgColor", "surface")] {
                                let color = get(attr);
                                if color.len() == 6 && color.bytes().all(|b| b.is_ascii_hexdigit()) {
                                    colors.insert(token.into(), format!("#{color}"));
                                }
                            }
                            if !colors.is_empty() {
                                report
                                    .preferences
                                    .insert("theme.overrides".into(), Preference::Colors(colors));
                            }
                            if !get("fontName").is_empty() {
                                report
                                    .preferences
                                    .insert("editor.font.family".into(), Preference::Text(get("fontName").into()));
                            }
                            if let Ok(size) = get("fontSize").parse::<i64>() {
                                if (6..=72).contains(&size) {
                                    report
                                        .preferences
                                        .insert("editor.font.size".into(), Preference::Integer(size));
                                }
                            }
                        }
                    }
                    "Shortcut" => {
                        // Stable Notepad++ command IDs; mapping is intentionally limited to direct equivalents.
                        let command = match get("id") {
                            "41001" => Some("file.new"),
                            "41002" => Some("file.open"),
                            "41003" => Some("file.close"),
                            "41006" => Some("file.save"),
                            "43001" => Some("search.find"),
                            "43003" => Some("search.replace"),
                            _ => None,
                        };
                        let key = get("Key")
                            .parse::<u32>()
                            .ok()
                            .and_then(char::from_u32)
                            .filter(|c| c.is_ascii_alphanumeric());
                        if let (Some(command), Some(key)) = (command, key) {
                            let mut chord = String::new();
                            for (attr, prefix) in [("Ctrl", "Ctrl+"), ("Alt", "Alt+"), ("Shift", "Shift+")] {
                                if get(attr) == "yes" {
                                    chord.push_str(prefix);
                                }
                            }
                            chord.push(key);
                            if report.shortcuts.len() < 128 {
                                report.shortcuts.push((command, chord));
                            }
                        } else if report.notes.len() < 256 {
                            report
                                .notes
                                .push(format!("Skipped shortcut {}: unsupported command/key", get("id")));
                        }
                    }
                    "File" | "HistoryFile" => {
                        let path = if get("filename").is_empty() {
                            get("name")
                        } else {
                            get("filename")
                        };
                        let b = path.as_bytes();
                        if b.len() > 3
                            && b[0].is_ascii_alphabetic()
                            && b[1] == b':'
                            && matches!(b[2], b'\\' | b'/')
                            && !path.contains('\0')
                            && !path[2..].contains(':')
                            && report.paths.len() < 256
                        {
                            report.paths.push(path.into());
                        } else if !path.is_empty() && report.notes.len() < 256 {
                            report
                                .notes
                                .push("Skipped nonlocal/device/relative session path".into());
                        }
                    }
                    "PluginCommands" | "Plugin" | "Macro" | "Command" => {
                        if report.notes.len() < 256 {
                            report
                                .notes
                                .push(format!("Skipped {tag}: executable/plugin state is never imported"));
                        }
                    }
                    _ => (),
                }
                if is_start {
                    depth += 1;
                    if depth > 32 {
                        return Err("Import nesting limit".into());
                    }
                }
            }
            Event::End(_) => {
                depth = depth.checked_sub(1).ok_or("Unbalanced XML")?;
            }
            Event::Text(ref text) if depth == 0 && text.as_ref().iter().any(|b| !b.is_ascii_whitespace()) => {
                return Err("Text outside XML root".into());
            }
            Event::CData(_) if depth == 0 => return Err("CDATA outside XML root".into()),
            Event::Eof => break,
            _ => (),
        }
    }
    if depth != 0 || roots != 1 {
        return Err("Incomplete XML document".into());
    }
    report.paths.sort();
    report.paths.dedup();
    Ok(report)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn safe_mapping_and_hostile_input() {
        let report=parse(br#"<NotepadPlus><GUIConfig name="TabSetting" size="4" replaceBySpace="yes"/><File filename="\\evil\x"/><File filename="C:\local.txt"/></NotepadPlus>"#).unwrap();
        assert_eq!(report.paths, vec!["C:\\local.txt"]);
        assert!(matches!(
            report.preferences.get("editor.tab.width"),
            Some(Preference::Integer(4))
        ));
        assert!(parse(b"<!DOCTYPE x><x/>").is_err());
        assert!(parse(b"junk<x/>").is_err());
        assert!(parse(b"<x/>junk").is_err());
        assert!(parse(b"<x><y></x>").is_err());
    }
}
