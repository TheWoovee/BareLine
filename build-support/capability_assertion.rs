// SPDX-License-Identifier: MPL-2.0
// Public marker used to bind built executables to the capability manifest.

pub const ASSERTION: &str = concat!(
    "BARELINE-CAPABILITY|component=",
    env!("BARELINE_BUILD_COMPONENT"),
    "|mode=",
    env!("BARELINE_BUILD_MODE"),
    "|config-version=",
    env!("BARELINE_CONFIG_VERSION"),
    "|config=",
    env!("BARELINE_CONFIG_DIGEST"),
    "|source=",
    env!("BARELINE_SOURCE_DIGEST"),
    "|version=",
    env!("BARELINE_RELEASE_VERSION"),
    "|features=",
    env!("BARELINE_BUILD_FEATURES"),
);

pub fn retain() {
    std::hint::black_box(ASSERTION);
}
