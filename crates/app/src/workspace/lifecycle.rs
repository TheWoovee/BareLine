// SPDX-License-Identifier: MPL-2.0
//! The file lifecycle of a tab (P6-02, ARC-02). Each tab holds one
//! [`FileLifecycle`], and every change to it goes through [`transition`],
//! which refuses the changes that broke recovery and file ownership before:
//! a save while the text is still loading, a second save, closing while a save
//! holds the file's lease, a compare view becoming a file's document
//! (REC-13), and replacing unsaved text whose recovery was not discarded
//! first (REC-04).
//!
//! External changes, a missing file and save conflicts are not lifecycle
//! states: the shell's watcher shows them as banners over a loaded tab, and a
//! conflict is kept as a record beside it. Read-only is the user's choice on
//! any state that owns text.

/// Where a tab's document stands in its file's life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileLifecycle {
    /// No tab: before a tab is created and after it closes. Closing releases
    /// a resident document's open-file lease (WSP-03); nothing else happens to
    /// a closed tab until Restore Closed Tab brings it back.
    Closed,
    /// An unsaved document without a file: a new tab, piped text, a missing
    /// launch path that its first save creates, or adopted recovered text.
    Untitled,
    /// An open, retry or resume shows this read-only tab while its file
    /// loads, holding a prefix at most. It cannot be saved; closing it cancels
    /// the open and remembers only the path (WSP-04).
    Loading,
    /// The open failed: an empty read-only placeholder showing the error with
    /// Retry and large-file actions (FIO-01). Closing remembers only the path.
    Failed,
    /// Bound to a file: its path, fingerprint and open-file lease.
    Loaded,
    /// A save of this tab is queued. The tab cannot close, so its file keeps
    /// its lease until the save settles, and a second save is refused. The
    /// save returns the tab to `from` unless it binds the tab to its file.
    Saving { from: SaveOrigin },
    /// A compare, historical or recovery view of text owned elsewhere. It is
    /// never dirty or bound to a file, is not in Save All, and neither owns
    /// nor discards a recovery journal (REC-13). It saves only as a copy.
    Preview,
}

/// The state a queued save started from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveOrigin {
    Untitled,
    Loaded,
    Preview,
}
impl SaveOrigin {
    fn state(self) -> FileLifecycle {
        match self {
            Self::Untitled => FileLifecycle::Untitled,
            Self::Loaded => FileLifecycle::Loaded,
            Self::Preview => FileLifecycle::Preview,
        }
    }
}

impl FileLifecycle {
    /// A save of this tab is queued.
    pub fn saving(self) -> bool {
        matches!(self, Self::Saving { .. })
    }
    /// The tab's text is its own: it may be dirty, saved by Save All, protected
    /// by a recovery journal and discarded. A preview's text belongs to another
    /// document (REC-13).
    pub fn owns_text(self) -> bool {
        !matches!(
            self,
            Self::Closed
                | Self::Preview
                | Self::Saving {
                    from: SaveOrigin::Preview
                }
        )
    }
    /// The tab may be moved out of memory under budget pressure: an automatic
    /// spill rebuilds clean text from the bound file, so only a loaded tab with
    /// no save in flight qualifies.
    pub fn spill_eligible(self) -> bool {
        self == Self::Loaded
    }
    /// Whether the state agrees with the tab as it is: a file is attached
    /// exactly while the tab is bound, and a tab that holds no text of its own
    /// (loading, failed, preview) is never dirty.
    pub(super) fn consistent(self, facts: TabFacts) -> bool {
        match self {
            Self::Closed => true,
            Self::Loaded
            | Self::Saving {
                from: SaveOrigin::Loaded,
            } => facts.bound,
            Self::Untitled
            | Self::Saving {
                from: SaveOrigin::Untitled,
            } => !facts.bound,
            Self::Loading
            | Self::Failed
            | Self::Preview
            | Self::Saving {
                from: SaveOrigin::Preview,
            } => !facts.bound && !facts.dirty,
        }
    }
    /// The state a tab whose creation was refused falls back to, so it never
    /// stays `Closed` while open.
    pub(super) fn settled(facts: TabFacts) -> Self {
        if facts.bound { Self::Loaded } else { Self::Untitled }
    }
}

/// Something that happens to a tab's file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LifecycleEvent {
    /// A new untitled tab.
    Created,
    /// A preview tab of text owned elsewhere.
    Previewed,
    /// An open shows the tab while it loads: a new loading tab, or a failed or
    /// loading tab that a retry, reopen or resume takes over.
    LoadStarted,
    /// The open failed and the tab keeps its error.
    LoadFailed,
    /// A file's text became the tab's document: an open finished in its own
    /// or a new tab, or Reload or Interpret As replaced the text in place.
    /// `unretired_edits` is set when the replaced text had unsaved edits whose
    /// recovery was not durably discarded first (REC-04).
    Opened { unretired_edits: bool },
    /// A save of the tab was queued; a copy leaves the tab's binding alone.
    SaveStarted { copy: bool },
    /// The queued save settled; `bound` when it bound the tab to the saved file.
    SaveSettled { bound: bool },
    /// Restore Closed Tab brought the tab back; `view` for a retained preview.
    Restored { view: bool },
    /// The tab closed.
    Closed,
}

/// What the workspace observes about a tab, as it is after an event.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct TabFacts {
    /// A file (path and fingerprint) is attached to the tab.
    pub(super) bound: bool,
    /// The tab's text differs from its saved or loaded state.
    pub(super) dirty: bool,
}

/// Why [`transition`] refused an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LifecycleRefusal {
    /// The event cannot happen in this state.
    Illegal,
    /// Only complete text that the tab owns can be saved: nothing is saved
    /// while it loads, and a preview saves only as a copy.
    NotReady,
    /// A save of this tab is already queued.
    AlreadySaving,
    /// The tab cannot close while its save holds the file's lease.
    SaveInFlight,
    /// Text with unsaved edits is replaced only after its recovery was
    /// durably discarded, as closing it would (REC-04).
    UnretiredEdits,
    /// The tab as observed contradicts the state the event leads to.
    Inconsistent,
}

/// The one transition function for a tab's file lifecycle. Returns the next
/// state, or why `event` cannot happen to a tab in `state` that will look like
/// `facts` afterwards. A refused event changes nothing.
pub(super) fn transition(
    state: FileLifecycle,
    event: LifecycleEvent,
    facts: TabFacts,
) -> Result<FileLifecycle, LifecycleRefusal> {
    use FileLifecycle as S;
    use LifecycleEvent as E;
    let next = match (state, event) {
        (S::Closed, E::Created) => S::Untitled,
        (S::Closed, E::Previewed) => S::Preview,
        (S::Closed | S::Loading | S::Failed, E::LoadStarted) => S::Loading,
        (S::Closed | S::Loading | S::Failed, E::LoadFailed) => S::Failed,
        (S::Closed | S::Untitled | S::Loading | S::Failed | S::Loaded, E::Opened { unretired_edits }) => {
            if unretired_edits {
                return Err(LifecycleRefusal::UnretiredEdits);
            }
            S::Loaded
        }
        (S::Saving { .. }, E::SaveStarted { .. }) => return Err(LifecycleRefusal::AlreadySaving),
        (S::Loading | S::Failed | S::Preview, E::SaveStarted { copy: false })
        | (S::Loading | S::Failed, E::SaveStarted { copy: true }) => return Err(LifecycleRefusal::NotReady),
        (S::Untitled, E::SaveStarted { .. }) => S::Saving {
            from: SaveOrigin::Untitled,
        },
        (S::Loaded, E::SaveStarted { .. }) => S::Saving {
            from: SaveOrigin::Loaded,
        },
        (S::Preview, E::SaveStarted { copy: true }) => S::Saving {
            from: SaveOrigin::Preview,
        },
        // A preview saves only as a copy, so its save never binds it.
        (
            S::Saving {
                from: SaveOrigin::Untitled | SaveOrigin::Loaded,
            },
            E::SaveSettled { bound: true },
        ) => S::Loaded,
        (S::Saving { from }, E::SaveSettled { bound: false }) => from.state(),
        (S::Closed, E::Restored { view }) => match (facts.bound, view) {
            (true, false) => S::Loaded,
            (false, false) => S::Untitled,
            (false, true) => S::Preview,
            (true, true) => return Err(LifecycleRefusal::Inconsistent),
        },
        (S::Saving { .. }, E::Closed) => return Err(LifecycleRefusal::SaveInFlight),
        (S::Untitled | S::Loading | S::Failed | S::Loaded | S::Preview, E::Closed) => S::Closed,
        _ => return Err(LifecycleRefusal::Illegal),
    };
    if next.consistent(facts) {
        Ok(next)
    } else {
        Err(LifecycleRefusal::Inconsistent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATES: [FileLifecycle; 9] = [
        FileLifecycle::Closed,
        FileLifecycle::Untitled,
        FileLifecycle::Loading,
        FileLifecycle::Failed,
        FileLifecycle::Loaded,
        FileLifecycle::Saving {
            from: SaveOrigin::Untitled,
        },
        FileLifecycle::Saving {
            from: SaveOrigin::Loaded,
        },
        FileLifecycle::Saving {
            from: SaveOrigin::Preview,
        },
        FileLifecycle::Preview,
    ];
    const EVENTS: [LifecycleEvent; 13] = [
        LifecycleEvent::Created,
        LifecycleEvent::Previewed,
        LifecycleEvent::LoadStarted,
        LifecycleEvent::LoadFailed,
        LifecycleEvent::Opened { unretired_edits: false },
        LifecycleEvent::Opened { unretired_edits: true },
        LifecycleEvent::SaveStarted { copy: false },
        LifecycleEvent::SaveStarted { copy: true },
        LifecycleEvent::SaveSettled { bound: false },
        LifecycleEvent::SaveSettled { bound: true },
        LifecycleEvent::Restored { view: false },
        LifecycleEvent::Restored { view: true },
        LifecycleEvent::Closed,
    ];
    const FACTS: [TabFacts; 4] = [
        TabFacts {
            bound: false,
            dirty: false,
        },
        TabFacts {
            bound: false,
            dirty: true,
        },
        TabFacts {
            bound: true,
            dirty: false,
        },
        TabFacts {
            bound: true,
            dirty: true,
        },
    ];

    /// SplitMix64: a fixed seed gives the same sequence on every run.
    struct Rng(u64);
    impl Rng {
        fn next_u64(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut value = self.0;
            value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            value ^ (value >> 31)
        }
        fn pick<T: Copy>(&mut self, values: &[T]) -> T {
            values[(self.next_u64() % values.len() as u64) as usize]
        }
    }

    /// The rules every accepted or refused step must obey, whatever led to it.
    fn check_step(state: FileLifecycle, event: LifecycleEvent, facts: TabFacts) -> FileLifecycle {
        let result = transition(state, event, facts);
        let context = format!("{state:?} --{event:?}/{facts:?}--> {result:?}");
        match result {
            Ok(next) => {
                // Binding, and the clean text of tabs that own none.
                assert!(next.consistent(facts), "{context}");
                // No save while loading, and never two saves at once.
                if matches!(event, LifecycleEvent::SaveStarted { .. }) {
                    assert!(
                        matches!(
                            state,
                            FileLifecycle::Untitled | FileLifecycle::Loaded | FileLifecycle::Preview
                        ),
                        "{context}"
                    );
                }
                // The lease is held while saving: a save ends only by settling.
                if state.saving() {
                    assert!(matches!(event, LifecycleEvent::SaveSettled { .. }), "{context}");
                }
                // A preview never loads, opens or binds (REC-13).
                if state == FileLifecycle::Preview {
                    assert!(
                        matches!(
                            next,
                            FileLifecycle::Closed
                                | FileLifecycle::Saving {
                                    from: SaveOrigin::Preview
                                }
                        ),
                        "{context}"
                    );
                }
                // Recovery: unretired edits are never replaced (REC-04).
                assert_ne!(event, LifecycleEvent::Opened { unretired_edits: true }, "{context}");
                // Only a closed tab is created or restored, and a closed tab stays closed
                // until then.
                if matches!(
                    event,
                    LifecycleEvent::Created | LifecycleEvent::Previewed | LifecycleEvent::Restored { .. }
                ) || state == FileLifecycle::Closed
                {
                    assert!(
                        state == FileLifecycle::Closed
                            && matches!(
                                event,
                                LifecycleEvent::Created
                                    | LifecycleEvent::Previewed
                                    | LifecycleEvent::LoadStarted
                                    | LifecycleEvent::LoadFailed
                                    | LifecycleEvent::Opened { .. }
                                    | LifecycleEvent::Restored { .. }
                            ),
                        "{context}"
                    );
                }
                // Text owned elsewhere never becomes the tab's own.
                if !state.owns_text() && state != FileLifecycle::Closed {
                    assert!(!next.owns_text(), "{context}");
                }
                next
            }
            // A refused step changes nothing, for the reason its state gives.
            Err(refusal) => {
                let expected = match (state, event) {
                    (FileLifecycle::Loading | FileLifecycle::Failed, LifecycleEvent::SaveStarted { .. }) => {
                        Some(LifecycleRefusal::NotReady)
                    }
                    (FileLifecycle::Saving { .. }, LifecycleEvent::SaveStarted { .. }) => {
                        Some(LifecycleRefusal::AlreadySaving)
                    }
                    (FileLifecycle::Saving { .. }, LifecycleEvent::Closed) => Some(LifecycleRefusal::SaveInFlight),
                    _ => None,
                };
                if let Some(expected) = expected {
                    assert_eq!(refusal, expected, "{context}");
                }
                state
            }
        }
    }

    #[test]
    fn random_transition_sequences_keep_every_invariant() {
        for seed in 0..64 {
            let mut rng = Rng(seed);
            let mut state = FileLifecycle::Closed;
            let mut visited = std::collections::BTreeSet::new();
            for _ in 0..2_000 {
                let event = rng.pick(&EVENTS);
                // Mostly the facts the state implies, so long legal runs happen;
                // sometimes arbitrary ones, which must be refused or consistent.
                let facts = if rng.next_u64().is_multiple_of(4) {
                    rng.pick(&FACTS)
                } else {
                    let bound = match event {
                        LifecycleEvent::Opened { .. } | LifecycleEvent::SaveSettled { bound: true } => true,
                        LifecycleEvent::Restored { view } => !view && rng.next_u64().is_multiple_of(2),
                        _ => {
                            matches!(
                                state,
                                FileLifecycle::Loaded
                                    | FileLifecycle::Saving {
                                        from: SaveOrigin::Loaded
                                    }
                            ) && !matches!(event, LifecycleEvent::Closed)
                        }
                    };
                    TabFacts { bound, dirty: false }
                };
                state = check_step(state, event, facts);
                visited.insert(format!("{state:?}"));
            }
            assert!(visited.len() >= 6, "seed {seed} explored only {visited:?}");
        }
    }

    #[test]
    fn every_pair_outside_the_table_is_refused() {
        use FileLifecycle as S;
        use LifecycleEvent as E;
        let legal = |state: S, event: E| match (state, event) {
            (S::Closed, E::Created | E::Previewed | E::LoadStarted | E::LoadFailed | E::Restored { .. }) => true,
            (S::Loading | S::Failed, E::LoadStarted | E::LoadFailed) => true,
            (S::Closed | S::Untitled | S::Loading | S::Failed | S::Loaded, E::Opened { unretired_edits }) => {
                !unretired_edits
            }
            (S::Untitled | S::Loaded, E::SaveStarted { .. }) => true,
            (S::Preview, E::SaveStarted { copy }) => copy,
            (S::Saving { from }, E::SaveSettled { bound }) => !bound || from != SaveOrigin::Preview,
            (S::Untitled | S::Loading | S::Failed | S::Loaded | S::Preview, E::Closed) => true,
            _ => false,
        };
        for state in STATES {
            for event in EVENTS {
                let accepted = FACTS.iter().any(|facts| transition(state, event, *facts).is_ok());
                assert_eq!(accepted, legal(state, event), "{state:?} --{event:?}-->");
            }
        }
    }

    #[test]
    fn nothing_is_saved_while_it_loads_or_saves() {
        let facts = TabFacts::default();
        for state in [FileLifecycle::Loading, FileLifecycle::Failed] {
            for copy in [false, true] {
                assert_eq!(
                    transition(state, LifecycleEvent::SaveStarted { copy }, facts),
                    Err(LifecycleRefusal::NotReady)
                );
            }
        }
        for from in [SaveOrigin::Untitled, SaveOrigin::Loaded, SaveOrigin::Preview] {
            assert_eq!(
                transition(
                    FileLifecycle::Saving { from },
                    LifecycleEvent::SaveStarted { copy: true },
                    facts
                ),
                Err(LifecycleRefusal::AlreadySaving)
            );
        }
    }

    #[test]
    fn a_saving_tab_cannot_close_and_keeps_its_lease() {
        let bound = TabFacts {
            bound: true,
            dirty: true,
        };
        let saving = FileLifecycle::Saving {
            from: SaveOrigin::Loaded,
        };
        assert_eq!(
            transition(saving, LifecycleEvent::Closed, bound),
            Err(LifecycleRefusal::SaveInFlight)
        );
        assert_eq!(
            transition(saving, LifecycleEvent::SaveSettled { bound: false }, bound),
            Ok(FileLifecycle::Loaded)
        );
        // A failed Save As leaves the document untitled; a finished one binds it.
        let untitled = FileLifecycle::Saving {
            from: SaveOrigin::Untitled,
        };
        assert_eq!(
            transition(
                untitled,
                LifecycleEvent::SaveSettled { bound: false },
                TabFacts::default()
            ),
            Ok(FileLifecycle::Untitled)
        );
        assert_eq!(
            transition(untitled, LifecycleEvent::SaveSettled { bound: true }, bound),
            Ok(FileLifecycle::Loaded)
        );
    }

    #[test]
    fn a_preview_never_binds_loads_or_owns_text() {
        let unbound = TabFacts::default();
        let preview = FileLifecycle::Preview;
        assert!(!preview.owns_text());
        for event in [
            LifecycleEvent::Opened { unretired_edits: false },
            LifecycleEvent::LoadStarted,
            LifecycleEvent::LoadFailed,
            LifecycleEvent::SaveStarted { copy: false },
        ] {
            assert!(transition(preview, event, unbound).is_err(), "{event:?}");
        }
        let copying = transition(preview, LifecycleEvent::SaveStarted { copy: true }, unbound).unwrap();
        assert!(!copying.owns_text());
        assert!(
            transition(
                copying,
                LifecycleEvent::SaveSettled { bound: true },
                TabFacts {
                    bound: true,
                    dirty: false
                }
            )
            .is_err()
        );
        assert_eq!(
            transition(copying, LifecycleEvent::SaveSettled { bound: false }, unbound),
            Ok(preview)
        );
        // A retained preview restores as a preview, never as a file's document.
        assert_eq!(
            transition(FileLifecycle::Closed, LifecycleEvent::Restored { view: true }, unbound),
            Ok(preview)
        );
        assert!(
            transition(
                FileLifecycle::Closed,
                LifecycleEvent::Restored { view: true },
                TabFacts {
                    bound: true,
                    dirty: false
                }
            )
            .is_err()
        );
        // A dirty preview contradicts the state.
        assert_eq!(
            transition(
                FileLifecycle::Closed,
                LifecycleEvent::Previewed,
                TabFacts {
                    bound: false,
                    dirty: true
                }
            ),
            Err(LifecycleRefusal::Inconsistent)
        );
    }

    #[test]
    fn replacing_unsaved_text_needs_its_recovery_discarded_first() {
        let loaded = TabFacts {
            bound: true,
            dirty: false,
        };
        assert_eq!(
            transition(
                FileLifecycle::Loaded,
                LifecycleEvent::Opened { unretired_edits: true },
                loaded
            ),
            Err(LifecycleRefusal::UnretiredEdits)
        );
        assert_eq!(
            transition(
                FileLifecycle::Loaded,
                LifecycleEvent::Opened { unretired_edits: false },
                loaded
            ),
            Ok(FileLifecycle::Loaded)
        );
    }

    #[test]
    fn the_state_must_match_the_files_binding() {
        assert_eq!(
            transition(
                FileLifecycle::Closed,
                LifecycleEvent::Opened { unretired_edits: false },
                TabFacts::default()
            ),
            Err(LifecycleRefusal::Inconsistent)
        );
        assert_eq!(
            transition(
                FileLifecycle::Closed,
                LifecycleEvent::Created,
                TabFacts {
                    bound: true,
                    dirty: false
                }
            ),
            Err(LifecycleRefusal::Inconsistent)
        );
        assert_eq!(
            transition(
                FileLifecycle::Closed,
                LifecycleEvent::Restored { view: false },
                TabFacts {
                    bound: true,
                    dirty: true
                }
            ),
            Ok(FileLifecycle::Loaded)
        );
        assert!(
            !FileLifecycle::Saving {
                from: SaveOrigin::Loaded
            }
            .spill_eligible()
        );
        assert!(FileLifecycle::Loaded.spill_eligible());
    }
}
