// SPDX-License-Identifier: MPL-2.0
pub use bareline_unicode_fold::character;
pub struct Unit {
    pub byte: u8,
    pub start: usize,
    pub end: usize,
    pub first: bool,
    pub last: bool,
}
pub struct MappedBytes<'a> {
    text: &'a str,
    base: usize,
    cursor: usize,
    folded: bool,
    buffer: [u8; 12],
    used: usize,
    position: usize,
    start: usize,
}
impl<'a> MappedBytes<'a> {
    pub fn new(text: &'a str, base: usize, folded: bool) -> Self {
        Self {
            text,
            base,
            cursor: 0,
            folded,
            buffer: [0; 12],
            used: 0,
            position: 0,
            start: 0,
        }
    }
}
impl Iterator for MappedBytes<'_> {
    type Item = Unit;
    fn next(&mut self) -> Option<Unit> {
        if !self.folded {
            let byte = *self.text.as_bytes().get(self.cursor)?;
            let start = self.base + self.cursor;
            self.cursor += 1;
            return Some(Unit {
                byte,
                start,
                end: start + 1,
                first: true,
                last: true,
            });
        }
        if self.position == self.used {
            let c = self.text[self.cursor..].chars().next()?;
            self.start = self.base + self.cursor;
            self.cursor += c.len_utf8();
            let mut temporary = [0; 12];
            let mapped = character(c, &mut temporary).as_bytes();
            self.used = mapped.len();
            self.buffer[..self.used].copy_from_slice(mapped);
            self.position = 0;
        }
        let result = Unit {
            byte: self.buffer[self.position],
            start: self.start,
            end: self.base + self.cursor,
            first: self.position == 0,
            last: self.position + 1 == self.used,
        };
        self.position += 1;
        Some(result)
    }
}
