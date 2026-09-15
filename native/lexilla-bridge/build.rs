// SPDX-License-Identifier: MPL-2.0
fn main() {
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++17")
        .warnings(false)
        .include("bundled/scintilla/include")
        .include("bundled/lexilla/include")
        .include("bundled/lexilla/lexlib")
        .file("src/bridge.cpp");
    for entry in std::fs::read_dir("bundled/lexilla/lexlib").unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "cxx") {
            build.file(path);
        }
    }
    for name in ["CPP", "Python", "Rust", "HTML", "CSS", "JSON", "SQL", "TOML"] {
        build.file(format!("bundled/lexilla/lexers/Lex{name}.cxx"));
    }
    build.flag_if_supported("/EHsc").compile("bareline_lexilla");
    println!("cargo:rerun-if-changed=src/bridge.cpp");
    println!("cargo:rerun-if-changed=bundled");
}
