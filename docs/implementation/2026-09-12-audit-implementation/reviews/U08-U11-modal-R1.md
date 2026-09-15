# U08/U11 integration correction — compare modal pane isolation

The integrated workspace test still expected the retired flat editor node while Compare Options was open. Split Compare publishes two stable U08 pane providers instead; U01 modal composition already makes every node outside the modal subtree disabled, non-focusable, and non-invokable.

The regression now checks both actual pane provider IDs while the modal owns focus, then dismisses the modal from editor-owned focus and verifies focus returns to the selected pane provider with both pane providers enabled again. No production modal, dock, or provider behavior changed.

Root-owned focused check: `cargo test -p bareline --bin bareline windows_app::accessibility::tests::prompt_and_compare_modals_own_focus_text_and_background_actions -- --exact`
