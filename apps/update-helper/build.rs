// SPDX-License-Identifier: MPL-2.0
#[path = "../../build-support/release_config.rs"]
mod release_config;

fn main() {
    release_config::configure("update-helper");
    // Static imports resolve from System32 only, never the user-writable directory
    // holding this verified executable (SEC-16, LOAD_LIBRARY_SEARCH_SYSTEM32).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        println!("cargo:rustc-link-arg-bin=bareline-update-helper=/DEPENDENTLOADFLAG:0x800");
    }
}
