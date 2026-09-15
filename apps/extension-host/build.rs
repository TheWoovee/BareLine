// SPDX-License-Identifier: MPL-2.0
#[path = "../../build-support/release_config.rs"]
mod release_config;

fn main() {
    release_config::configure("extension-runtime");
}
