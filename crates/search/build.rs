// SPDX-License-Identifier: MPL-2.0
fn main() {
    let directory = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let source = "data/UnicodeData-17.0.0.txt";
    println!("cargo:rerun-if-changed={source}");
    let mut ranges = vec![(0x200Cu32, 0x200Du32)];
    let mut first = None;
    for line in std::fs::read_to_string(source).unwrap().lines() {
        let fields: Vec<_> = line.split(';').collect();
        let code = u32::from_str_radix(fields[0], 16).unwrap();
        let word = fields[2].starts_with(['L', 'N', 'M']) || fields[2] == "Pc";
        if fields[1].ends_with(", First>") {
            first = Some(code);
            continue;
        }
        if fields[1].ends_with(", Last>") {
            let start = first.take().expect("Unicode range start");
            if word {
                ranges.push((start, code));
            }
        } else if word {
            ranges.push((code, code));
        }
    }
    ranges.sort_unstable();
    let mut merged: Vec<(u32, u32)> = Vec::new();
    for (start, end) in ranges {
        if let Some(last) = merged.last_mut()
            && start <= last.1 + 1
        {
            last.1 = last.1.max(end);
            continue;
        }
        merged.push((start, end));
    }
    let mut output =
        String::from("// Generated Unicode 17.0.0 categories; Unicode-3.0.\nstatic WORD: &[(u32,u32)] = &[\n");
    for (start, end) in merged {
        output.push_str(&format!("({start},{end}),\n"));
    }
    output.push_str("];\n");
    std::fs::write(directory.join("word.rs"), output).unwrap();
}
