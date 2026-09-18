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
    let mut library_sources: Vec<_> = std::fs::read_dir("bundled/lexilla/lexlib")
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "cxx"))
        .collect();
    library_sources.sort();
    build.files(library_sources);
    for name in ["CPP", "Python", "Rust", "HTML", "CSS", "JSON", "SQL", "TOML"] {
        build.file(format!("bundled/lexilla/lexers/Lex{name}.cxx"));
    }
    let compiler = build.get_compiler();
    if compiler.is_like_msvc() && !compiler.is_like_clang() {
        // MSVC includes source paths in anonymous-namespace RTTI identifiers.
        // /Brepro alone does not normalize those paths across build roots.
        let root = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        build.flag(format!("/d1trimfile:{root}\\"));
    }
    build.flag_if_supported("/EHsc").compile("bareline_lexilla");
    println!("cargo:rerun-if-changed=src/bridge.cpp");
    println!("cargo:rerun-if-changed=bundled");
}
