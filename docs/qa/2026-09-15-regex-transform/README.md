# Regex transform — focused evidence, 2026-09-15

**Result:** native regex_transform s1/s2/s3 and checked editor Exit 0 pass. [Implementation note](../../implementation/production-readiness/DEV-003-REGEX-TRANSFORM.md).

The app exposes its existing bounded preview rows/status to accessibility. The adapter observes a native checked Regex menu, multiline query and exact match count, named/numbered capture expansions before mutation, one open-document apply, exact saved bytes and single Undo restoring the original bytes. The final owned client screenshot is in `runs/native-regex-r7/scratch/regex-preview.png`.

## Candidate and scope

- Debug SHA-256: `7fecdbf149a4dc073db6fa791944b92296aa204c6b179fc063ea73870605a8d7`.
- Native cell: Windows 10.0.26200, software renderer, dark private profile, keyboard/native commands, 96 DPI. Generated data only, private profile, owned Job cleanup.
- Final run: `22d6c06d-406d-49ab-ac9a-72cdf15b93a7`; 18.865 seconds, 3 PASS steps and clean Exit 0.
- 41 focused Python tests (9 regex, 22 adapter, 10 code/config) and one focused Rust test pass. The regex tests were repeated after changing the mode oracle. Only the named Rust test executed; 159 others were filtered out.
- Incremental debug app build: 13.790 seconds. The initial test-target compile failed on the new accessibility import path; the correction and passing test are separately captured. No full suites, aggregate journeys or release build.

## Retention and failed attempts

[verification-summary.json](verification-summary.json) lists the exact receipts, runs and limits. [retained-files.json](retained-files.json) maps original paths to retained paths with byte counts and SHA-256. `captures/` contains unmodified capture receipts/stdout/stderr; `runs/` contains complete generated run data; `source/` contains inert copies of the final touched/required source files. Source hashes and dirty-tree identities belong to their recorded executions and must not be relabelled as later executions.

All seven failed native attempts remain: native-regex stopped on foreground loss before typing; r1 hit unavailable field TextPattern geometry; r2/r3 exposed the harness's mode-control name/setup assumptions; r4 lacked the initial Editor provider inside three seconds; r5 identified the Folder edit; r6 confirmed Enter does not submit that folder picker. r7 passes after bounded driver corrections. Only the first is a foreground stop; none was a locked desktop or a passing product run. The final code uses field ValuePattern, observes the checked native Regex menu, waits at most five seconds for initial Editor publication, and activates the owned Select Folder button once.

The generated fixture has two matches, named and numbered capture references, combining text, emoji, CJK and unchanged lines. Source and restored file: 79 bytes; transformed snapshot: 61 bytes. Preview leaves both text and bytes untouched. Apply changes one open document with no implicit disk write; native Save writes the exact transformed bytes, and one Undo plus Save restores the original bytes.

## Remaining qualification

This is one small resident-document journey using an isolated folder containing its single open input file. It does not qualify large/paged/chunk-boundary regex, all PCRE2 compatibility cases, cancellation, stale-source aborts, multi-document groups, closed-file rollback, other environments or physical accessibility. Preview row/action focus and the preexisting generic accessible mode-control label remain under broader QUAL-024. No AC mapping/import or capability promotion occurred. **47 top-level items remain open; ten DEV-003 procedures remain unimplemented.**
