// SPDX-License-Identifier: MPL-2.0
fn main() {
    let source = "data/CaseFolding-17.0.0.txt";
    println!("cargo:rerun-if-changed={source}");
    let input = std::fs::read_to_string(source).expect("vendored Unicode case folding data");
    let mut output = String::from(
        "// Generated from Unicode 17.0.0; Unicode-3.0, see data/LICENSE-UNICODE.\nstatic FOLD: &[(char, &str)] = &[\n",
    );
    for line in input.lines() {
        let body = line.split('#').next().unwrap().trim();
        if body.is_empty() {
            continue;
        }
        let fields: Vec<_> = body.split(';').map(str::trim).collect();
        if !matches!(fields[1], "C" | "F") {
            continue;
        }
        let escaped: String = fields[2]
            .split_whitespace()
            .map(|code| format!("\\u{{{code}}}"))
            .collect();
        output.push_str(&format!("('\\u{{{}}}', \"{}\"),\n", fields[0], escaped));
    }
    output.push_str("];\n");
    let directory = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    std::fs::write(directory.join("case_folding.rs"), output).unwrap();
}
