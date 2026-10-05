// SPDX-License-Identifier: MPL-2.0
include!(concat!(env!("OUT_DIR"), "/case_folding.rs"));

pub fn character(c: char, buffer: &mut [u8; 12]) -> &str {
    if c.is_ascii() {
        return c.to_ascii_lowercase().encode_utf8(buffer);
    }
    if let Ok(index) = FOLD.binary_search_by_key(&c, |entry| entry.0) {
        FOLD[index].1
    } else {
        c.encode_utf8(buffer)
    }
}
/// Full default, locale-independent Unicode 17.0.0 case folding.
pub fn fold(text: &str) -> String {
    let mut out = String::new();
    let mut buffer = [0u8; 12];
    for c in text.chars() {
        out.push_str(character(c, &mut buffer));
    }
    out
}
#[cfg(test)]
mod tests {
    #[test]
    fn expansions_and_sigma() {
        assert_eq!(
            super::fold("Stra\u{df}e \u{3a3}\u{3c2} \u{fb03}"),
            "strasse \u{3c3}\u{3c3} ffi"
        );
    }
}
