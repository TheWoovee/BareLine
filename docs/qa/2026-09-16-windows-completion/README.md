# Windows completion verification — 2026-09-16

Actual local executions, not production approval. Work was sequential. Source/build/native/synthetic/retained evidence have separate recorded identities.

## Results

- Full workspace/all-target Rust execution: one stale semantic label snapshot failed; all other targets passed. The reviewed change updates 22 intended Extended Search labels only, and the focused rerun passes. Ignored tests were not executed by that command.
- Exact Clippy ratchet passes with 755 observed reviewed occurrences, no new/stale debt; one obsolete baseline entry was removed. This is not a zero-warning claim.
- Python: 113 E2E tooling + 14 release script + 21 performance tooling + 3 soak tooling tests passed (151 total across these distinct groups).
- Static checks: 64 Python files, 3 workflow YAML files, 34 PowerShell scripts, formatting, portability and the pinned toolchain contract pass.
- Dependency policy: advisories/bans/licenses/sources pass, with recorded duplicate/unused-allowance warnings. No separate cargo-audit/sanitizer campaign is claimed.
- Optimized editor, update helper and extension host build passed (5m13s). Real WASI components and release-host Fast tests pass 3/3.
- Third-party and SDK notices generate successfully after retaining 15 omitted license texts from the exact locked upstream commit.
- Typed soak-supervisor evidence was executed and revalidated from retained objects. This is 3 headless supervisor checks, not a native soak or AC acceptance.

## Native boundary

Plain text passed all steps earlier in this pass. Seven new journey procedures are authored, but none gained full native acceptance here. Column testing exposed an actual fallback-font measurement defect, now fixed with a passing real DirectWrite regression. Its final native rerun stopped on lost foreground before steps. Historical failures and their owned-window artifacts are retained. Earlier runs pointing at mutable target/debug bytes are retained as opaque history when those binary bytes no longer match; no new binary identity is assigned. Current immutable native failure inputs are retained with exact references.

## Retention

Manifest pin: `59b26ae8eaf44f9b3041a73292627882c05debf6daeee95320d121059a5799fa`.

Retained 673 files; 89391287 cumulative input bytes. Objects deduplicate identical content.

[Collection manifest pointer](retained-v2/bundle.json), [byte verification](verification-v2.json), [producer verification](producer-verification-v2.json), [readiness report](readiness-v2.json), [execution summary](execution-summary.json).

`verify` reads only retained objects. `verify-producers` replays supported typed observations and lists native observations it does not recompute. Unit regressions additionally remove original caches before typed replay. No final reviewer identity or release closure is attested. Original caches have not been deleted as part of retaining this real run. The first retained bundle remains byte-valid but omitted the backlog role, so its readiness report is incomplete; retained-v2 corrects the collection plan without overwriting the first bundle.

```powershell
python tests/e2e/evidence_bundle.py verify --bundle docs/qa/2026-09-16-windows-completion/retained-v2 --expected-sha256 59b26ae8eaf44f9b3041a73292627882c05debf6daeee95320d121059a5799fa
python tests/e2e/evidence_bundle.py verify-producers --bundle docs/qa/2026-09-16-windows-completion/retained-v2 --expected-sha256 59b26ae8eaf44f9b3041a73292627882c05debf6daeee95320d121059a5799fa
```

## Captures

A successful wrapper can select zero tests; receipt 01 is explicitly not counted. Nonzero and source-drift receipts are preserved rather than overwritten. A changed-source receipt cannot qualify its intended source boundary; the later consolidated captures (55 onward) have stable source identities. Receipt 76 is the expected incomplete prerequisite report, not an implementation regression.

| Capture | Exit | Seconds | Source stable |
|---|---:|---:|---|
| [01-catalog-authority.json](retained-v2/objects/b0e23d9e4668c0075936371a673071cf9525ac989de43df8024c6fedffae5244) | 0 | 8.095 | True |
| [02-catalog-authority.json](retained-v2/objects/ba5287a3be5916418dba028f04695181d9bff8c3114d85c397d0ae78ae2903db) | 0 | 0.366 | True |
| [03-root-policy.json](retained-v2/objects/fa3e71243b63c20e15a810604e0f7b2418cf79856c8fc17e5439635861d8c064) | 0 | 5.891 | False |
| [04-native-root-ledger.json](retained-v2/objects/204605c53d0b896c61cdf623b0f29933e0ec44708ba148c525ea005b0a0e2353) | 0 | 14.307 | True |
| [05-native-root-floor.json](retained-v2/objects/0bf06f614d0800b47ac2148c354da44f395c8fe5f7ea06191f811eb95b9b9a78) | 0 | 4.750 | True |
| [06-authority-wiring.json](retained-v2/objects/7c2c904ff92341c0dce2756279f9b1f9287efa9a072ac5451cd0b63556af79ef) | 0 | 18.251 | True |
| [07-authority-verifier.json](retained-v2/objects/a0c50c8689780f2c355d891fbe75dec104d50c5f8472757d0fa188ee7a5e0dde) | 0 | 1.289 | True |
| [08-authority-package.json](retained-v2/objects/bfbc54a9fd2522dde00df54813abb8c7f3b1e08cc5fbd4506c0611e521296524) | 0 | 5.393 | True |
| [09-package-preview-regression.json](retained-v2/objects/677ec437eadb98be3067aeb41807f91184d1f250004aabb57ff4a2c82cefda21) | 1 | 0.493 | True |
| [10-package-preview-pwsh.json](retained-v2/objects/0d4b888848917f0e907738a76f6b4e6cc4b8924270e85f25cc8a536400cb2673) | 0 | 0.874 | True |
| [11-release-pipeline.json](retained-v2/objects/94a96d0e082d9750af20086e80b93931c7bb7cb1265100c990a49ea86ff2245a) | 0 | 0.428 | True |
| [12-delivery-verifier-build.json](retained-v2/objects/1659d696fbc66059a9680c318eefd804fea33bfd2e0206253894b00cce71d6d8) | 0 | 8.550 | False |
| [13-evidence-runner-regression.json](retained-v2/objects/21bff1737f8427deb9523a7fb2d7858c1e56809e201d440f08c2e64d1f16e580) | 0 | 0.673 | True |
| [14-environment-regression.json](retained-v2/objects/bcbf0e6c056e7cc79bd9cb30be5b355c2cf876307504b8c0c48f902856ec41ec) | 0 | 1.125 | True |
| [15-typed-producers.json](retained-v2/objects/0f504554c444d41ecedc88e0ae9f2e275a5159c1775e9b94bbbc9c99a1aa5d99) | 1 | 2.417 | True |
| [16-typed-producers-crlf.json](retained-v2/objects/bc4092265c3b600f3a0baa98b14dd46d4ca4a9fc9946feab87326612f3c4770a) | 0 | 2.530 | True |
| [17-typed-performance.json](retained-v2/objects/92ea74f162679ea474a630e1dd9f712ca94fa89ba24e6e23d88085f312327511) | 0 | 2.318 | True |
| [18-closure.json](retained-v2/objects/bf8a4154f29026a19f8f54d897d0c812c65395bdb1bf457789757cd15546fe30) | 1 | 0.337 | True |
| [19-retained-typed.json](retained-v2/objects/ee83913fe469e9a04e99ad4899043cec3d489a364577329aee0d5acae532f368) | 0 | 3.081 | True |
| [20-closure-green.json](retained-v2/objects/c1811583dd75ddc6db55cbbca3a2fd5da2fae00b976649ef754f874ae4db421a) | 0 | 0.341 | True |
| [21-bundle-regression.json](retained-v2/objects/45b74ede99929d6a2270895ec37f3118e29051686307126fed08c35c3ebe5b8b) | 0 | 3.708 | True |
| [22-authority-nonshipping-pins.json](retained-v2/objects/48bcc2757011309d7007d456664f2dad40b61099eba552789d20a6840538e78c) | 0 | 5.442 | True |
| [23-package-final-layout.json](retained-v2/objects/cf8128e2961021e81ed592aeee8644e0e926518ccb104e31e2acbca50197c441) | 0 | 0.841 | True |
| [24-language-catalog.json](retained-v2/objects/debb47cb19b8402f6f97f385be8777192df36fc89f9c02c2b45e9ce38a6ba0ad) | 101 | 14.679 | True |
| [25-language-catalog.json](retained-v2/objects/c803af71531938c8b25fa20d8744adf4ac3d83afa14d1d6fcfba4f96c408c536) | 0 | 23.047 | True |
| [26-windows-fixture-oracles.json](retained-v2/objects/d1d61ced4cc43cbebaffcbac9fe187e6d9106e463c1fe0108d6a5edadd1c6f87) | 0 | 0.138 | True |
| [27-python-e2e.json](retained-v2/objects/47d4a9a4d348835a50c69ea67e9f73e58f348c0fa280062406e3c7dfa8642def) | 1 | 9.991 | True |
| [28-python-typed.json](retained-v2/objects/5fe0ba05a407ca8d3a556abebe3d18af4fbb5e3229eced5c33dc9816fd689872) | 0 | 3.003 | True |
| [29-windows-build.json](retained-v2/objects/7d1cbac5f43fb1e6a485022cdb45a49f57fdf9a044c14565d7bbd4966c895a8a) | 101 | 77.029 | True |
| [30-windows-build.json](retained-v2/objects/896d698d7b21907fb52601835d4eaf111122caa29c6c85805eae4cb6b03362c5) | 0 | 39.850 | True |
| [31-native-plain.json](retained-v2/objects/d18fff15993617c3bf501be42be4a364726a33d7903ece7c7ebe50164b4e6bbd) | 0 | 21.216 | True |
| [32-native-column.json](retained-v2/objects/d5ac48d1d169b80aec9bb58a37617f4bd785b8eda4165168848d5758bc8eb670) | 1 | 14.347 | True |
| [33-native-column.json](retained-v2/objects/9aff51db7924a87e99bc0e4e2a4da665eda507b1e1d83ed4390f981536448314) | 1 | 14.048 | True |
| [34-native-column.json](retained-v2/objects/14e95e8a35cf772b5c7b81faa93abebd729fbcfde20260bb3dbc4cdb11e24b85) | 1 | 13.081 | True |
| [35-native-column.json](retained-v2/objects/f821be204657df316c66db5894dbdb4c8ab5e5374603c16dda5baaad68be2ee6) | 1 | 13.805 | True |
| [36-native-column.json](retained-v2/objects/6b9c46b2ca5d675910fcbafa6747e20ac5455bd07d0e282a6d043e84c09ef132) | 1 | 16.110 | True |
| [37-native-column.json](retained-v2/objects/a8f9d85822fc79c75ce92868ba128c24827a4be70dc30831126c16764c7a36f1) | 1 | 14.901 | True |
| [38-key-trace-build.json](retained-v2/objects/85fe50a8f284a2d5df99803cfa9b57077474537ecafd294372fe7493fff14a34) | 0 | 35.006 | True |
| [39-native-column-trace.json](retained-v2/objects/dd7a80624318440ad91e0e0cd29e4dd3d572d71cb8c772c73cd517c20aaff8c3) | 1 | 24.518 | True |
| [40-native-column.json](retained-v2/objects/a6af1cc9288e6f692f958ad1fbe178b9823efb23064f841825e51f1159ad713e) | 1 | 17.862 | True |
| [41-remove-key-trace-build.json](retained-v2/objects/e230c2fa2771271c064b99b95a50cf6bf88b604482b3cc3417ea21eb29bae5e4) | 0 | 8.062 | True |
| [42-native-column.json](retained-v2/objects/899bc04823d26481fef6f6334af842210696c90a4fd28364f5222f0e138d0cc7) | 1 | 18.130 | True |
| [43-native-column.json](retained-v2/objects/0831e9e7eac1a5dd3e504844a27385c82c6dcdf269d3876bf58ab71655ec4925) | 1 | 20.455 | True |
| [44-native-column.json](retained-v2/objects/f358ea423ad19695b3f1e2fde16c6190c0c5f5d158cef9ee9404afcba608dfa8) | 1 | 17.726 | True |
| [45-native-column.json](retained-v2/objects/bd0e9f6ef5f971267070fa199dbbc6173527dc35d489ef9c48bf962c9d9e821f) | 1 | 16.258 | True |
| [46-native-column-geometry.json](retained-v2/objects/2d9269e71bf9af1fbeab5f407b67ba2ef5ce99a06d6f53b9f6084bdc03e2125e) | 101 | 0.199 | True |
| [47-native-column-geometry.json](retained-v2/objects/32265a58209bc725c87c62ba3d7ae4aa30ada6b3705fec879ced5dd942299a6c) | 0 | 12.059 | True |
| [48-column-fix-build.json](retained-v2/objects/9b03f61d71e5de34053794f5e54cfcf3b15e830f06af02a301d7722d4249ac65) | 0 | 13.874 | True |
| [49-native-column.json](retained-v2/objects/fa6a10af63b6b643465acf0503e9ca1c67f2668bf968ffbd7137ed2d306fa095) | 1 | 7.266 | True |
| [50-typed-capture-regression.json](retained-v2/objects/312bb83cc1b042ddae8cf0f692c481750992130def3afd6f04443acb347a76e7) | 0 | 3.604 | True |
| [51-capture-self-test.json](retained-v2/objects/03f8c3ba6b12ace30553d40c95990e29ee5fc0aaa80195085eda20c6eac425b1) | 0 | 2.140 | True |
| [52-release-python.json](retained-v2/objects/8547ed7e4777dca8bba4d71425041098e5aa4b9e1ad068d455fe644fea0d1470) | 0 | 11.722 | True |
| [53-handoff-retention.json](retained-v2/objects/3832fd15c01339a22c2af8c836d696ff964adb51fb11e5222f03699f34dfdba1) | 0 | 0.636 | True |
| [54-definition-tests.json](retained-v2/objects/fdf8a6345912e7185150742c7ef664c180fd0151879f9f4a56f7b2bf222b5e8d) | 0 | 0.330 | True |
| [55-e2e-python.json](retained-v2/objects/f64af94100042cb9efc4563354f9f518d9ce1363f5439a017835b7770649b8bc) | 0 | 11.158 | True |
| [56-workspace-tests.json](retained-v2/objects/24bb7c7a59f6ff1e92ff56e760b13011973d82f525f4226626959a63b6dc1b79) | 101 | 332.686 | True |
| [57-semantic-golden.json](retained-v2/objects/89dc629a7ea62cb078bfc8793a9c56ac49fb4c6a3c8e6878296906f2ead2b2b5) | 0 | 30.685 | True |
| [58-clippy-census.json](retained-v2/objects/3178506588b1b9f88982cd00863374c47af257a3d7cf57e8f2a52552a3f81bc5) | 0 | 35.493 | True |
| [59-clippy-ratchet.json](retained-v2/objects/2c3203cd11df0704ea91bedbf045d7d726a53ceaeec6c23a051acdc473119e01) | 1 | 34.869 | True |
| [60-clippy-census-fixed.json](retained-v2/objects/e043d8cd65284cf59c8d29b262f5774a10a2681fd49c3d7581eef62a53061786) | 0 | 3.612 | True |
| [61-clippy-ratchet-fixed.json](retained-v2/objects/487249d67342f3ff6ff2cb4c36fbbe941c5ff996bc4cd1998436f854802154a1) | 0 | 26.298 | True |
| [62-script-tests.json](retained-v2/objects/d5f0c5ea53000c81fa187809522052a3aeb19a6062b47141e8bdb135280999a0) | 0 | 9.756 | True |
| [63-perf-tools.json](retained-v2/objects/611902904aaac80ab48f0f802ff9562aa50c5f4de7eb63d4239ff7e89cdbc593) | 0 | 3.757 | True |
| [64-soak-tools.json](retained-v2/objects/5d5d15c4df971e7f3caa0b10bbc1c40d7c6bfc88d8402415fe38259741bce499) | 0 | 0.227 | True |
| [65-static-python-yaml.json](retained-v2/objects/05eb3130b237bd37da5a1c74735c429170d79b376df1e1fe56a75e3a97256b4a) | 0 | 0.479 | True |
| [66-powershell-syntax.json](retained-v2/objects/8d16bcee562d3340fab303981e1dd5185f18e6351dbf9ebcd175d3436dd99bf8) | 0 | 1.083 | True |
| [67-rustfmt.json](retained-v2/objects/51cfbe003f40b8385fd7268cbd7cdb5bb24c0456848da4ab51cee01ccc3dbaff) | 0 | 3.350 | True |
| [68-portability.json](retained-v2/objects/fc45dc1fb1815c899e1ea1ed3091e5e1588d51b0553d555579d795bdf363ab30) | 0 | 0.628 | True |
| [69-toolchain.json](retained-v2/objects/744f5b35c7a674280742c961fae0b4a886736f76829f5449afa5c3d6ae79de5d) | 0 | 0.226 | True |
| [70-dependency-policy.json](retained-v2/objects/aef8c374a9d435e0735ad9ee2807621969b5b58bcbb74cc5a429058a80c54429) | 0 | 8.936 | True |
| [71-release-build.json](retained-v2/objects/76407c9ff6f9277bf606ce4d89c4e2c7e9776ac9d2bc6ee2a4651d97e212f767) | 0 | 313.690 | True |
| [72-first-party-fast.json](retained-v2/objects/7b95d914e650c92630ebd12a943f88a5a22c38ee6331483e6aa78c70069cb168) | 1 | 261.279 | True |
| [73-first-party-fast-fixed.json](retained-v2/objects/84b2006dd5e6fb4319ff8f7267c20d7feff8d21482a46ea9c3504e6a6d7b0f3c) | 0 | 19.260 | True |
| [74-release-notices.json](retained-v2/objects/2d7a5784c3559e32cb123447a8848b137baa6bf66f654ff93602390efe033a0a) | 1 | 5.365 | True |
| [75-powershell-final.json](retained-v2/objects/7299c7fb39bbecdac73e9ab40b49e30a3cb4db5cd75bb3ddd3f35ff9bd3cf1e4) | 0 | 0.832 | True |
| [76-prerequisite-report.json](retained-v2/objects/cb61bd6e4804c5d870a044a00a9eecc321dab2ad57d968d355184aa4ab019eae) | 1 | 0.377 | True |
| [77-release-notices-fixed.json](retained-v2/objects/884682c8e422ad96f74cc782a19bf45f0f7419697457e600061ea5be31870ae0) | 0 | 3.153 | True |
| [78-typed-soak-proof.json](retained-v2/objects/ef50b0be44040b4afc546b951b70e9b734b8175555c954c04699c5eefed825b1) | 0 | 1.539 | True |

## Remaining gates

The live queue is **7 implemented/dispositioned, 41 active, 1 owner-deferred**. DEV-003/004/006 retain explicit driver/mapping/soak work. Real production public trust/signing, clean Windows-floor VMs, physical input/AT/DPI, the 72-hour run and independent final review remain. Linux/macOS qualification is deferred by the owner. See [current status](../../IMPLEMENTATION_STATUS.md).

Automatic approval review rejected preparation of the Inno Setup compiler installation command with only “blocked by policy” as its reason. Installer compilation remains pending; no product installation or publication occurred.
