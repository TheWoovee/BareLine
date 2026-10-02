// SPDX-License-Identifier: MPL-2.0
//! User-facing error language and the error-surface policy (UI-03, ARC-04).
//!
//! # Wording
//!
//! Every error enum that can reach status text, a banner, a dialog, a notification
//! or a tab placeholder has one plain-language sentence, kept next to the enum as
//! its `Display` implementation (or a `user_message`/`label` method for enums whose
//! `Display` would be ambiguous). The match is exhaustive, so a new variant does not
//! compile until it has wording. The sentence says what happened, whether the
//! user's text is safe, and what the user can do next. `Debug` output (`{error:?}`)
//! names internal variants and is for the diagnostics log (`eprintln!("event=…")`)
//! only; this module's tests fail when production code of the UI-facing crates
//! formats a value with `Debug`, and when a mapped message reads like an identifier.
//!
//! # Surfaces
//!
//! Choose where an error appears by its [`Severity`]:
//!
//! - [`Severity::Transient`]: one request was refused, stopped or cancelled and
//!   nothing is lost (busy, try again, invalid input). Status text
//!   (`Workspace::message`, a panel status) or a transient toast; never a dialog.
//! - [`Severity::Document`]: one document cannot currently be shown, edited, saved
//!   or recovered as expected. The document's banner (`EditorSurface::error`, the
//!   failed-open tab placeholder) or a persistent toast bound to that document, so
//!   the notice stays until the document recovers or closes.
//! - [`Severity::Decision`]: work cannot continue until the user chooses (discard,
//!   overwrite, keep both versions). A modal dialog, and only in this case. Render
//!   and layout failures are never modal (APP-08); an error never exits the app.
//!
//! Technical detail that helps support but not the user (variant names, OS error
//! codes, worker names) goes to the diagnostics log or a toast's details, never
//! into the headline text.
//!
//! The UI-03 wording changes kept each message on the surface it already used.
//! Routing existing call sites through [`Severity::surface`] (and moving any site
//! whose surface disagrees with its severity) is the P1-E6 follow-up.

/// How much an error disrupts the user, which decides its [`Surface`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// A single request did not complete; nothing is lost and it can be retried.
    Transient,
    /// One document cannot be shown, edited, saved or recovered as expected.
    Document,
    /// Work cannot continue until the user makes a choice.
    Decision,
}

/// Where an error is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    /// Status text or a transient toast.
    Status,
    /// A banner or persistent notice tied to the affected document.
    Banner,
    /// A modal dialog that asks for a decision.
    Modal,
}

impl Severity {
    /// The one surface this severity uses (ARC-04).
    pub const fn surface(self) -> Surface {
        match self {
            Self::Transient => Surface::Status,
            Self::Document => Surface::Banner,
            Self::Decision => Surface::Modal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// Lists every unit variant of an enum. The `match` has no wildcard, so adding
    /// a variant fails to compile here until the test covers its wording.
    macro_rules! unit_variants {
        ($target:ty: $($variant:ident),+ $(,)?) => {{
            type Target = $target;
            let _exhaustive = |value: &Target| match value {
                $(Target::$variant)|+ => (),
            };
            vec![$(Target::$variant),+]
        }};
    }

    /// True for a word such as `WrongDocument` or `budgetExceeded`. Binary size
    /// units (`64 MiB`, `256KiB`) are the product's wording for limits, not names.
    fn identifier_like(word: &str) -> bool {
        let word = word.trim_start_matches(|c: char| c.is_ascii_digit());
        !matches!(word, "KiB" | "MiB" | "GiB" | "TiB")
            && word
                .chars()
                .zip(word.chars().skip(1))
                .any(|(first, second)| first.is_lowercase() && second.is_uppercase())
    }

    fn assert_plain(message: &str, debug: &str) {
        assert!(!message.trim().is_empty(), "{debug} has no message");
        assert_ne!(message, debug, "{debug} is shown in its Debug form");
        assert!(
            !message.contains(['{', '}']) && !message.contains("Some(") && !message.contains("None"),
            "{message:?} reads like Debug output of {debug}"
        );
        for word in message.split(|c: char| !c.is_alphanumeric()) {
            assert!(
                !identifier_like(word),
                "{message:?} names the identifier {word:?} (from {debug})"
            );
        }
    }

    fn check<T: std::fmt::Debug + std::fmt::Display>(values: impl IntoIterator<Item = T>) {
        for value in values {
            assert_plain(&value.to_string(), &format!("{value:?}"));
        }
    }

    fn io_error() -> std::io::Error {
        std::io::Error::from(std::io::ErrorKind::PermissionDenied)
    }

    #[test]
    fn document_errors_have_plain_language() {
        use bareline_document::{
            Error, line_lookup::LineLookupPoll, paged::IndexError, service::SubmitError, source::Unavailable,
        };
        check(unit_variants!(Error:
            OutOfBounds, InvalidBoundary, OverlappingEdits, StaleRevision, BudgetExceeded, EmptyHistory,
            WrongDocument, RevisionOverflow, IncompleteSource, EmptyTransaction, LinkedUndoRequired, ActorBusy,
        ));
        check(unit_variants!(IndexError: StaleSnapshot, OutOfOrder, Cancelled, WindowTooLarge));
        check(unit_variants!(Unavailable: SourceChanged, InvalidRange, Cancelled));
        check(unit_variants!(SubmitError: Saturated, Closed, InvalidGroup));
        for poll in [
            LineLookupPoll::Unavailable(Unavailable::SourceChanged),
            LineLookupPoll::Failed(Error::WrongDocument),
            LineLookupPoll::Cancelled,
            LineLookupPoll::Finished,
        ] {
            assert_plain(poll.failure_message(), &format!("{poll:?}"));
        }
        // The edit status names the limit, never the variant (UI-03).
        let status = format!("Edit was not applied: {}.", Error::BudgetExceeded);
        assert!(!status.contains("BudgetExceeded"), "{status}");
    }

    #[test]
    fn file_errors_have_plain_language() {
        use bareline_file_io::{
            CodecError,
            codecs::{disk::DiskError, failure::EncodingFailure, resident::ResidentError},
            lifecycle::{
                CommitFailure, FileError, Fingerprint, PostCommitCancellation, PostCommitConflict,
                PostCommitVerification, SaveConflict,
            },
            paged_service::PagedLifecycleError,
            session::SessionIssue,
        };
        let codec = || {
            vec![
                CodecError::InvalidSequence,
                CodecError::Unrepresentable,
                CodecError::UnresolvedOpaqueBytes,
                CodecError::Output(io_error()),
            ]
        };
        check(codec());
        let disk = || {
            let mut values = vec![
                DiskError::At {
                    range: 3..5,
                    reason: "the text has characters this encoding cannot store".into(),
                },
                DiskError::Io(io_error()),
                DiskError::Budget,
                DiskError::Changed,
                DiskError::Cancelled,
                DiskError::Quota {
                    used: 1,
                    required: 2,
                    limit: 3,
                },
                DiskError::NotComplete,
                DiskError::Failed,
            ];
            values.extend(codec().into_iter().map(DiskError::Codec));
            values
        };
        check(disk());
        let mut resident = vec![
            ResidentError::At {
                range: 3..5,
                reason: "the text has characters this encoding cannot store".into(),
            },
            ResidentError::Cancelled,
            ResidentError::Limit,
            ResidentError::DirtyInterpret,
            ResidentError::WrongDocument,
        ];
        resident.extend(codec().into_iter().map(ResidentError::Codec));
        resident.push(ResidentError::Document(bareline_document::Error::IncompleteSource));
        check(resident);
        let path = || PathBuf::from("notes.txt");
        let fingerprint = || Fingerprint {
            identity: bareline_platform::FileIdentity {
                volume: 1,
                file: 2,
                length: 3,
                modified: 4,
            },
            sha256: [0; 32],
        };
        let failure = || EncodingFailure::new((1, 1), 0..1, "the text has characters this encoding cannot store");
        let mut files = vec![
            FileError::EncodingAt(failure()),
            FileError::Encoding(ResidentError::WrongDocument),
            FileError::Cancelled,
            FileError::Io(io_error()),
            FileError::Changed,
            FileError::UnsupportedEncoding,
            FileError::StreamingRequired,
            FileError::IncompleteSource,
            FileError::Budget,
            FileError::Conflict {
                target: path(),
                proposed: path(),
                transaction: path(),
            },
            FileError::ConflictAfterCommit(Box::new(PostCommitConflict {
                target: path(),
                proposed: path(),
                displaced: path(),
                transaction: path(),
                approved: fingerprint(),
                actual_displaced: fingerprint(),
            })),
            FileError::ConflictAfterCreate {
                target: path(),
                proposed: path(),
                transaction: path(),
            },
            FileError::VerificationAfterCommit(Box::new(PostCommitVerification {
                target: path(),
                proposed: path(),
                displaced: None,
                transaction: path(),
                reason: "the saved bytes differ".into(),
            })),
            FileError::Commit(Box::new(CommitFailure {
                staged: path(),
                proposed: None,
                displaced: None,
                transaction: None,
                error: io_error(),
            })),
        ];
        for displaced in [None, Some(path())] {
            files.push(FileError::CancelledAfterCommit(Box::new(PostCommitCancellation {
                target: path(),
                proposed: path(),
                displaced,
                transaction: path(),
            })));
        }
        files.extend(disk().into_iter().map(FileError::Transcode));
        for file in &files {
            // Exhaustive without a wildcard: a new variant fails to compile here.
            match file {
                FileError::EncodingAt(_)
                | FileError::Transcode(_)
                | FileError::Encoding(_)
                | FileError::Cancelled
                | FileError::Io(_)
                | FileError::Changed
                | FileError::UnsupportedEncoding
                | FileError::StreamingRequired
                | FileError::IncompleteSource
                | FileError::Budget
                | FileError::Conflict { .. }
                | FileError::ConflictAfterCommit(_)
                | FileError::ConflictAfterCreate { .. }
                | FileError::CancelledAfterCommit(_)
                | FileError::VerificationAfterCommit(_)
                | FileError::Commit(_) => {}
            }
        }
        check(files);
        let lifecycle = vec![
            PagedLifecycleError::Busy,
            PagedLifecycleError::Changed,
            PagedLifecycleError::Cancelled,
            PagedLifecycleError::Encoding(failure()),
            PagedLifecycleError::Conflict(Box::new(SaveConflict {
                target: Some(path()),
                editor_version: path(),
                other_version: None,
                transaction: path(),
                state: bareline_platform::CommitState::Conflict,
                verified: false,
            })),
            // The detail names the stopped worker; it stays in Debug for diagnostics.
            PagedLifecycleError::SourceUnavailable("documentActor stopped".into()),
            PagedLifecycleError::Failed(FileError::Changed),
        ];
        for error in &lifecycle {
            // Exhaustive without a wildcard. `CleanupPending` owns a private retry
            // receipt and cannot be built outside file-io; its `Display` is one
            // fixed sentence.
            match error {
                PagedLifecycleError::Busy
                | PagedLifecycleError::Changed
                | PagedLifecycleError::Cancelled
                | PagedLifecycleError::Encoding(_)
                | PagedLifecycleError::Conflict(_)
                | PagedLifecycleError::SourceUnavailable(_)
                | PagedLifecycleError::CleanupPending(_)
                | PagedLifecycleError::Failed(_) => {}
            }
        }
        check(lifecycle);
        let issues = unit_variants!(SessionIssue:
            InvalidEntry, ResourceLimit, DuplicateField, DuplicateIdentity, InvalidReference, InvalidPath,
            InvalidView, PinOrder, InvalidLayout, InvalidComparison, ActiveFallback,
        );
        for issue in issues {
            assert_plain(issue.label(), &format!("{issue:?}"));
        }
    }

    #[test]
    fn search_syntax_and_compare_errors_have_plain_language() {
        use bareline_search::{
            Completeness, RegexLimitKind, ReplaceError, SearchMode, folders::FolderSkip, replace_disk::ReceiptState,
            replace_files::PreviewError, service::SearchError,
        };
        let mut completeness = vec![
            Completeness::Complete,
            Completeness::Cancelled,
            Completeness::ResultLimit,
            Completeness::Unsupported,
            Completeness::UnsupportedStreaming,
            Completeness::InvalidQuery,
        ];
        completeness.extend(
            unit_variants!(RegexLimitKind: Time, Backtracking, Depth, Memory, Engine)
                .into_iter()
                .map(Completeness::RegexLimit),
        );
        check(completeness);
        check(unit_variants!(SearchError: Superseded, Stopped));
        check(unit_variants!(FolderSkip:
            Untrusted, Symlink, Io, Changed, UnsupportedEncoding, SubjectLimit, Binary, DepthLimit, ResourceLimit,
        ));
        let replace = || {
            vec![
                ReplaceError::Incomplete,
                ReplaceError::Stale,
                ReplaceError::StagingLimit,
                ReplaceError::Cancelled,
                ReplaceError::NoMatch,
                ReplaceError::InvalidReplacement,
                ReplaceError::TooManyReplacements { count: 9, limit: 8 },
            ]
        };
        check(replace());
        let mut preview = vec![
            PreviewError::TooManyDocuments,
            PreviewError::DuplicateDocument,
            PreviewError::WrongDocument,
        ];
        preview.extend(replace().into_iter().map(PreviewError::Replace));
        check(preview);
        let reason = || "File is read-only; change its permissions and preview again".to_owned();
        check(vec![
            ReceiptState::Planned,
            ReceiptState::Staged,
            ReceiptState::Uncertain(reason()),
            ReceiptState::Committed,
            ReceiptState::ReconciledCommitted,
            ReceiptState::RollbackStaged,
            ReceiptState::RolledBack,
            ReceiptState::Skipped(reason()),
            ReceiptState::Failed(reason()),
            ReceiptState::Conflict,
            ReceiptState::RollbackFailed(reason()),
        ]);
        for mode in unit_variants!(SearchMode: Literal, Extended, Regex) {
            assert!(!identifier_like(mode.label()), "{mode:?}");
        }
        let syntax = unit_variants!(bareline_syntax::Error: Cancelled, InvalidRange, BudgetExceeded, StaleCheckpoint);
        for error in &syntax {
            assert_plain(bareline_syntax::udl::validation_message(*error), &format!("{error:?}"));
        }
        check(syntax);
        check(unit_variants!(bareline_syntax::SubmitError: Stopped));
        for kind in unit_variants!(bareline_syntax::udl::MappingKind: Imported, Approximated, Unsupported) {
            assert!(!identifier_like(kind.label()), "{kind:?}");
        }
        for kind in unit_variants!(bareline_syntax::outline::MappingKind: Imported, Approximated, Unsupported) {
            assert!(!identifier_like(kind.label()), "{kind:?}");
        }
        let apply = unit_variants!(bareline_diff::ApplyError:
            Unavailable, Stale, InvalidRange, BudgetExceeded, UnsupportedPreserve,
        );
        check(apply.clone());
        let mut compare = vec![
            crate::compare::CompareError::Busy,
            crate::compare::CompareError::WorkerUnavailable,
            crate::compare::CompareError::NoResult,
            crate::compare::CompareError::Stale,
            crate::compare::CompareError::MissingHunk,
            crate::compare::CompareError::InvalidSession,
        ];
        compare.extend(apply.into_iter().map(crate::compare::CompareError::Apply));
        check(compare);
    }

    #[test]
    fn platform_extension_and_app_errors_have_plain_language() {
        use bareline_extensions_protocol::{PackageError, broker::BrokerError};
        use bareline_macros::process::ProcessState;
        use bareline_platform::printing::PrintError;
        check(unit_variants!(bareline_renderer::LayoutError:
            InvalidHandle, InvalidOffset, ResourceLimit, BackendFailure,
        ));
        check(vec![
            PrintError::Cancelled,
            PrintError::Unavailable("Printer unavailable; select another printer and retry".into()),
            PrintError::Driver("Printer rejected the job; retry with another printer".into()),
            PrintError::InvalidOptions,
            PrintError::InvalidLine,
        ]);
        check(unit_variants!(bareline_platform::executor::SubmitError: Busy, Closed));
        check(unit_variants!(bareline_platform::PathDecodeError:
            UnsupportedVersion, ForeignPlatform, InvalidBase64, InvalidCodeUnits, TooLong,
        ));
        check(
            unit_variants!(bareline_platform::Capability:
                About, OpenFile, SaveFile, PickFolder, MenuBar, ContextMenu, Shell, Printing, Tray, Update,
                FileWatch,
            )
            .into_iter()
            .map(|capability| bareline_platform::Unsupported { capability }),
        );
        check(unit_variants!(bareline_extensions_protocol::ProtocolError:
            Io, Oversized, Malformed, Version, InvalidIdentity, ChunkLimit,
        ));
        check(unit_variants!(PackageError:
            OnlineUnavailable, InvalidSignature, WrongIdentity, HashMismatch, Metadata, Expired, Rollback, Io,
            Size, UnsafeArchive, AlreadyInstalled, Cancelled, UnsupportedCapability,
        ));
        check(unit_variants!(BrokerError:
            Denied, InvalidScope, PendingLimit, DuplicateRequest, UnknownRequest, Expired, StaleRevision,
            ChunkLimit, StagingLimit, InvalidEdits, ApprovalRequired,
        ));
        check(vec![
            bareline_commands::DispatchError::Unknown(bareline_commands::CommandId("file.open")),
            bareline_commands::DispatchError::Disabled("Document is read-only".into()),
        ]);
        check(unit_variants!(crate::utilities::UtilityError:
            Cancelled, InvalidRange, IncompleteSource, BudgetExceeded, InvalidDecode, InvalidUtf8, Io, StaleStyles,
        ));
        check(unit_variants!(crate::views::ViewError:
            Missing, InvalidPane, InvalidState, IdentityExhausted, LastDirtyReference, PinnedBoundary,
        ));
        let elapsed = std::time::Duration::from_millis(1500);
        for state in [
            ProcessState::Starting,
            ProcessState::Running,
            ProcessState::Exited { code: Some(0), elapsed },
            ProcessState::Exited { code: None, elapsed },
            ProcessState::Cancelled { elapsed },
            ProcessState::Failed("the program could not be started".into()),
        ] {
            let status = crate::macros::process_status(&state);
            assert_plain(&status, &format!("{state:?}"));
        }
        for confidence in unit_variants!(bareline_file_io::codecs::Confidence:
            Bom, Utf8Sample, Utf16Sample, LegacySample, LegacyFallback,
        ) {
            assert_plain(
                crate::encoding::confidence_label(confidence),
                &format!("{confidence:?}"),
            );
        }
    }

    #[test]
    fn only_a_decision_opens_a_modal() {
        assert_eq!(Severity::Transient.surface(), Surface::Status);
        assert_eq!(Severity::Document.surface(), Surface::Banner);
        assert_eq!(Severity::Decision.surface(), Surface::Modal);
    }

    #[test]
    fn plain_language_check_rejects_identifiers() {
        let caught = |message: &'static str, debug: &'static str| {
            std::panic::catch_unwind(|| assert_plain(message, debug)).is_err()
        };
        assert!(caught("Edit was not applied: BudgetExceeded", "BudgetExceeded"));
        assert!(caught("Recovery unavailable: WrongDocument", "WrongDocument"));
        assert!(caught("Quota { used: 1 }", "Quota { used: 1 }"));
        assert!(caught("Busy", "Busy"));
        assert!(!caught("The document is busy; try again in a moment", "Busy"));
        // Size units are words the user reads, not identifiers.
        assert!(!caught(
            "results incomplete: regex context exceeds 64 MiB",
            "UnsupportedStreaming"
        ));
        assert!(!caught("Outline definition exceeds 256KiB", "ResourceLimit"));
        assert!(caught("Limit reached: MaxMiB", "MaxMiB"));
    }

    /// Production sources of the crates whose strings reach the user.
    const SCANNED: &[&str] = &[
        "apps/bareline/src",
        "crates/app/src",
        "crates/commands/src",
        "crates/editor-surface/src",
        "crates/file-io/src",
        "crates/macros/src",
        "crates/platform/src",
        "crates/platform-windows/src",
        "crates/search/src",
        "crates/syntax/src",
        "crates/ui/src",
    ];
    /// Debug formatting that never reaches the user, with the reason.
    const ALLOWED: &[(&str, &str)] = &[
        // Keymap names are winit's key names ("ArrowUp", "KeyA"), not error text.
        ("apps/bareline/src/windows_app/settings.rs", "Key::Named(named)"),
        ("apps/bareline/src/windows_app/settings.rs", "PhysicalKey::Code(code)"),
        // Diagnostic payload: `PagedLifecycleError`'s `Display` never shows it.
        ("crates/file-io/src/paged_service.rs", "SourceUnavailable(format!("),
        // A broken internal invariant; the cache kind is for the diagnostics log.
        ("crates/file-io/src/owned_cache.rs", "kind={kind:?}"),
    ];
    /// Lines that write to the diagnostics log, assert, or only hash state.
    const DIAGNOSTIC: &[&str] = &[
        "eprintln!",
        "println!",
        "assert",
        "panic!",
        "unreachable!",
        "expect(",
        "event=",
        ".hash(",
    ];

    /// `{:?}`, `{name:?}` or `{name:#?}`.
    fn debug_placeholder(line: &str) -> bool {
        line.match_indices('{').any(|(start, _)| {
            let rest = &line[start + 1..];
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
                .unwrap_or(rest.len());
            let (name, tail) = rest.split_at(end);
            !name.starts_with(|c: char| c.is_ascii_digit()) && (tail.starts_with(":?}") || tail.starts_with(":#?}"))
        })
    }

    /// `#[cfg(test)]` or `#[cfg(all(test, …))]`: the item never ships.
    fn test_only_attribute(line: &str) -> bool {
        let Some(condition) = line
            .trim()
            .strip_prefix("#[cfg(")
            .and_then(|rest| rest.strip_suffix(")]"))
        else {
            return false;
        };
        condition == "test"
            || (condition.starts_with("all(")
                && !condition.contains("not(test")
                && condition
                    .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .any(|word| word == "test"))
    }

    /// The line and the lines before it that continue the same statement (at most
    /// three), so a diagnostic macro opened just above excuses it but a finished
    /// `expect(...);` statement above does not.
    fn statement<'a, 'b>(lines: &'a [&'b str], index: usize) -> &'a [&'b str] {
        let mut start = index;
        while start > 0 && index - start < 3 && !lines[start - 1].trim_end().ends_with([';', '{', '}']) {
            start -= 1;
        }
        &lines[start..=index]
    }

    /// Indices of lines outside test-only items, which run to the closing brace at
    /// their own indentation (rustfmt layout).
    fn production_lines(lines: &[&str]) -> Vec<usize> {
        let mut kept = Vec::new();
        let mut index = 0;
        while index < lines.len() {
            let line = lines[index];
            if test_only_attribute(line) {
                let indent = &line[..line.len() - line.trim_start().len()];
                index += 1;
                while index < lines.len() && lines[index].trim_start().starts_with("#[") {
                    index += 1;
                }
                if index < lines.len() && !lines[index].ends_with(';') {
                    let close = format!("{indent}}}");
                    while index < lines.len() && lines[index] != close {
                        index += 1;
                    }
                }
                index += 1;
                continue;
            }
            kept.push(index);
            index += 1;
        }
        kept
    }

    fn scan(root: &Path, path: &Path, findings: &mut Vec<String>) {
        let relative = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
        let source = std::fs::read_to_string(path).unwrap();
        let lines: Vec<&str> = source.lines().collect();
        for index in production_lines(&lines) {
            let line = lines[index];
            if line.trim_start().starts_with("//") || !debug_placeholder(line) {
                continue;
            }
            if statement(&lines, index)
                .iter()
                .any(|line| DIAGNOSTIC.iter().any(|token| line.contains(token)))
                || ALLOWED
                    .iter()
                    .any(|(file, text)| relative == *file && line.contains(text))
            {
                continue;
            }
            findings.push(format!("{relative}:{}: {}", index + 1, line.trim()));
        }
    }

    /// UI-03: user-facing text never carries `Debug` output. Map the value to its
    /// plain-language `Display` (or label) instead; log `Debug` only as diagnostics.
    #[test]
    fn ui_sources_do_not_debug_format_values() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let mut findings = Vec::new();
        for directory in SCANNED {
            let mut pending = vec![directory.split('/').fold(root.clone(), |path, part| path.join(part))];
            while let Some(directory) = pending.pop() {
                for entry in std::fs::read_dir(&directory).unwrap() {
                    let path = entry.unwrap().path();
                    let name = path.file_name().unwrap().to_string_lossy().into_owned();
                    if path.is_dir() {
                        if name != "tests" {
                            pending.push(path);
                        }
                    } else if name.ends_with(".rs") && !name.ends_with("tests.rs") {
                        scan(&root, &path, &mut findings);
                    }
                }
            }
        }
        findings.sort();
        assert!(
            findings.is_empty(),
            "Debug-formatted values in user-facing code; use the error's plain-language Display:\n{}",
            findings.join("\n")
        );
    }

    #[test]
    fn source_check_finds_debug_placeholders_outside_tests() {
        assert!(debug_placeholder(r#"format!("Edit was not applied: {error:?}")"#));
        assert!(debug_placeholder(r#"format!("{:?}", state)"#));
        assert!(debug_placeholder(r#"format!("{value:#?}")"#));
        assert!(!debug_placeholder(r#"format!("{error}")"#));
        assert!(!debug_placeholder(r#"format!("{0:?}")"#));
        let lines = [
            "fn shown() {",
            "    let text = format!(\"{error:?}\");",
            "}",
            "#[cfg(test)]",
            "mod tests {",
            "    fn hidden() { format!(\"{error:?}\"); }",
            "}",
            "#[cfg(test)]",
            "#[path = \"extra_tests.rs\"]",
            "mod extra;",
            "fn after() {}",
        ];
        assert_eq!(production_lines(&lines), vec![0, 1, 2, 10]);
        let lines = [
            "#[cfg(all(test, windows))]",
            "mod native_tests {",
            "    fn hidden() { format!(\"{error:?}\"); }",
            "}",
            "#[cfg(not(test))]",
            "fn shipped() {}",
        ];
        assert_eq!(production_lines(&lines), vec![4, 5]);
        assert!(!test_only_attribute("#[cfg(windows)]"));
        assert!(!test_only_attribute("#[cfg(any(test, feature = \"fixtures\"))]"));
        // A finished statement above does not excuse the line; an open diagnostic
        // macro does.
        let lines = [
            "let value = source.expect(\"loaded\");",
            "workspace.message = Some(format!(\"{value:?}\"));",
        ];
        assert_eq!(statement(&lines, 1), &lines[1..]);
        let lines = ["eprintln!(", "    \"event=save state={state:?}\","];
        assert_eq!(statement(&lines, 1), &lines[..]);
    }
}
