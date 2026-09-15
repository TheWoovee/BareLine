// SPDX-License-Identifier: MPL-2.0
//! Explicit extended-mode escapes; unknown/incomplete escapes are rejected.
pub fn decode(input: &str) -> Option<String> {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            output.push(c);
            continue;
        }
        output.push(match chars.next()? {
            '\\' => '\\',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            '0' => '\0',
            'x' => hexadecimal(&mut chars, 2)?,
            'u' => hexadecimal(&mut chars, 4)?,
            'U' => hexadecimal(&mut chars, 8)?,
            _ => return None,
        });
    }
    Some(output)
}
fn hexadecimal(chars: &mut impl Iterator<Item = char>, count: usize) -> Option<char> {
    let mut code = 0u32;
    for _ in 0..count {
        code = code.checked_mul(16)?.checked_add(chars.next()?.to_digit(16)?)?;
    }
    char::from_u32(code)
}
