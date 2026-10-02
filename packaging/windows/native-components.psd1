# SPDX-License-Identifier: MPL-2.0
# Native C and C++ code compiled into the packaged executables. Cargo metadata and
# CycloneDX output describe only the Rust crates that build it, so generate-notices.ps1
# and generate-sbom.ps1 read these identities. CargoPackage names the crate that
# compiles the component; CargoVersion, when set, must match Cargo.lock so a crate
# update cannot silently keep stale versions and license texts.
@{
    Components = @(
        @{
            Name = 'lexilla'
            Version = '5.5.3'
            License = 'License for Lexilla, Scintilla, and SciTE'
            LicenseFiles = @('native/lexilla-bridge/bundled/lexilla/License.txt')
            SourceDirectory = 'native/lexilla-bridge/bundled/lexilla'
            Cpe = 'cpe:2.3:a:scintilla:lexilla:5.5.3:*:*:*:*:*:*:*'
            Purl = 'pkg:github/ScintillaOrg/lexilla@rel-5-5-3'
            CargoPackage = 'bareline-lexilla-bridge'
            # The original subset has the same git blobs in rel-5-5-2 and rel-5-5-3;
            # the rest of lexers/ was imported from rel-5-5-3 to complete the set.
            Evidence = 'Bundled include/, lexlib/ and the complete lexers/ set (125 files) are byte-identical to ScintillaOrg/lexilla tag rel-5-5-3 (git blob ids match; native/lexilla-bridge/BUNDLED-SHA256.txt).'
        }
        @{
            Name = 'scintilla'
            Version = '5.5.7'
            License = 'License for Lexilla, Scintilla, and SciTE'
            LicenseFiles = @('native/lexilla-bridge/bundled/scintilla/License.txt')
            SourceDirectory = 'native/lexilla-bridge/bundled/scintilla'
            Cpe = 'cpe:2.3:a:scintilla:scintilla:5.5.7:*:*:*:*:*:*:*'
            CargoPackage = 'bareline-lexilla-bridge'
            # Interface headers only. They are identical in 5.5.7 and 5.5.8; 5.5.7 is
            # the version tests/perf/README-WINDOWS-ADAPTERS.md pins.
            Evidence = 'Bundled include/ headers are byte-identical to Scintilla release 5.5.7 (rel-5-5-7).'
        }
        @{
            Name = 'pcre2'
            Version = '10.46'
            License = 'BSD-3-Clause WITH PCRE2-exception'
            LicenseFiles = @('packaging/windows/native-licenses/pcre2-10.46/LICENCE.md')
            Cpe = 'cpe:2.3:a:pcre:pcre2:10.46:*:*:*:*:*:*:*'
            Purl = 'pkg:github/PCRE2Project/pcre2@pcre2-10.46'
            CargoPackage = 'pcre2-sys'
            CargoVersion = '0.2.10'
            Evidence = 'pcre2-sys 0.2.10 upstream/ sources (PCRE2_MAJOR 10, PCRE2_MINOR 46); see native-licenses/pcre2-10.46/provenance.md.'
        }
        @{
            Name = 'sljit'
            Version = 'e51eabbfb8eabc6526f56e4e88b29fb10d1ee048'
            License = 'BSD-2-Clause'
            LicenseFiles = @('packaging/windows/native-licenses/sljit-e51eabbf/LICENSE')
            # SLJIT has no releases, so the commit purl identifies it; PCRE2 JIT
            # advisories are published against the PCRE2 CPE above.
            Purl = 'pkg:github/zherczeg/sljit@e51eabbfb8eabc6526f56e4e88b29fb10d1ee048'
            CargoPackage = 'pcre2-sys'
            CargoVersion = '0.2.10'
            Evidence = 'PCRE2 10.46 deps/sljit submodule, compiled by pcre2-sys 0.2.10 with SUPPORT_JIT; see native-licenses/sljit-e51eabbf/provenance.md.'
        }
    )
}
