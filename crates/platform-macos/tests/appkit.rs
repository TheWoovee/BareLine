// SPDX-License-Identifier: MPL-2.0
//! AppKit checks that need the main thread, which the default test harness
//! never runs tests on: this target has `harness = false` and runs each
//! check from `main`. It needs a window-server session (GitHub's macOS
//! runners have one); off macOS it does nothing.
#[cfg(target_os = "macos")]
mod appkit {
    use bareline_commands::{
        Action, CommandContext, CommandId, CommandRegistry, CommandSpec, CommandState, Keymap, MenuItem, MenuModel,
    };
    use bareline_platform::{
        SaveDialogOptions, SaveFileKind,
        clipboard::{DEFAULT_CLIPBOARD_MAX_BYTES, RECTANGLE_CLIPBOARD_FORMAT},
    };
    use bareline_platform_macos::{
        CommandMessage, MacAppearance, MacClipboard, MacDialogs, MacMenuBar, MenuDispatch,
        prompts::{CANCEL_ID, FIRST_BUTTON_RETURN, InAppPrompt},
        system_ui_language,
        types::save_panel_plan,
    };
    use objc2_foundation::MainThreadMarker;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };

    fn registry() -> CommandRegistry {
        let mut registry = CommandRegistry::default();
        for (id, title, shortcut, action) in [
            ("file.save", "&Save", "Ctrl+S", Action::Save),
            ("app.quit", "Exit", "Alt+F4", Action::Quit),
            ("edit.copy", "Copy", "Ctrl+C", Action::Copy),
            ("help.about", "About Bareline", "F1", Action::About),
        ] {
            registry
                .register(CommandSpec {
                    id: CommandId(id),
                    title,
                    category: "Test",
                    shortcut,
                    action,
                })
                .unwrap();
        }
        registry
    }
    fn model() -> MenuModel {
        let id = |name| MenuItem::Command(CommandId(name));
        MenuModel {
            items: vec![
                MenuItem::Submenu {
                    title: "File".into(),
                    items: vec![id("file.save"), MenuItem::Separator, id("app.quit")],
                },
                MenuItem::Submenu {
                    title: "Edit".into(),
                    items: vec![id("edit.copy")],
                },
                MenuItem::Submenu {
                    title: "Help".into(),
                    items: vec![id("help.about")],
                },
            ],
        }
    }

    fn menu_bar_builds_dispatches_and_follows_state(mtm: MainThreadMarker) {
        let registry = registry();
        let (sender, receiver) = mpsc::channel();
        let woken = Arc::new(AtomicUsize::new(0));
        let wake = woken.clone();
        let mut bar = MacMenuBar::new(
            mtm,
            42,
            &registry,
            model(),
            MenuDispatch {
                sender,
                notify: Arc::new(move || {
                    wake.fetch_add(1, Ordering::Relaxed);
                }),
            },
        );
        // Application menu, File, Edit; Help is emptied by moving About.
        // SAFETY: reading a live menu on the main thread (as below).
        assert_eq!(unsafe { bar.main_menu().numberOfItems() }, 3);
        let titles = bar.item_titles();
        let title = |id| {
            let tag = bar.plan().tag(CommandId(id)).unwrap();
            titles
                .iter()
                .find(|(candidate, _)| *candidate == tag)
                .unwrap()
                .1
                .clone()
        };
        assert_eq!(title("file.save"), "Save");
        assert_eq!(title("app.quit"), "Quit Bareline");
        assert_eq!(title("help.about"), "About Bareline");

        // Choosing an item queues its tag for the window and wakes the loop.
        let save = bar.plan().tag(CommandId("file.save")).unwrap();
        // SAFETY: as above.
        let item = unsafe {
            bar.main_menu()
                .itemAtIndex(1)
                .unwrap()
                .submenu()
                .unwrap()
                .itemWithTag(save)
                .unwrap()
        };
        // SAFETY: sends the item's own action to its target on the main thread.
        unsafe {
            let target = item.target().unwrap();
            let _: () = objc2::msg_send![&target, barelineMenuCommand: &*item];
        }
        let message: CommandMessage = receiver.try_recv().unwrap();
        assert_eq!((message.hwnd, message.id, message.action), (42, save as usize, 0));
        assert!(bar.accepts_command(&message));
        assert_eq!(bar.action(message.id), Some(Action::Save));
        assert_eq!(woken.load(Ordering::Relaxed), 1);
        // SAFETY: as above.
        assert_eq!(unsafe { item.keyEquivalent() }.to_string(), "s");

        // A disabled command validates as disabled and dispatches nothing.
        let mut context = CommandContext::default();
        context
            .states
            .insert(CommandId("file.save"), CommandState::disabled("Nothing to save"));
        bar.sync_commands_localized(&registry, &context, &Keymap::defaults(&registry), 1, |_, fallback| {
            fallback.to_owned()
        });
        // SAFETY: as above.
        unsafe {
            let target = item.target().unwrap();
            let valid: bool = objc2::msg_send![&target, validateMenuItem: &*item];
            assert!(!valid);
            let _: () = objc2::msg_send![&target, barelineMenuCommand: &*item];
        }
        assert!(receiver.try_recv().is_err());

        // A structural change rebuilds the menus with the same tags.
        context
            .states
            .insert(CommandId("edit.copy"), CommandState::not_applicable("No selection"));
        bar.refresh_structure(&registry, &context);
        // SAFETY: as above.
        assert_eq!(unsafe { bar.main_menu().numberOfItems() }, 2);
        drop(bar);
        println!("ok menu_bar_builds_dispatches_and_follows_state");
    }

    fn private_pasteboard_round_trips_text_and_metadata(mtm: MainThreadMarker) {
        let clipboard = MacClipboard::private(mtm);
        assert_eq!(clipboard.text_within(DEFAULT_CLIPBOARD_MAX_BYTES).unwrap(), None);
        clipboard
            .set_text_with_metadata(
                "ab\ncd",
                DEFAULT_CLIPBOARD_MAX_BYTES,
                RECTANGLE_CLIPBOARD_FORMAT,
                b"rows",
            )
            .unwrap();
        let contents = clipboard
            .text_with_metadata(DEFAULT_CLIPBOARD_MAX_BYTES, RECTANGLE_CLIPBOARD_FORMAT, 64)
            .unwrap()
            .unwrap();
        assert_eq!(contents.text, "ab\ncd");
        assert_eq!(contents.metadata.as_deref(), Some(&b"rows"[..]));
        assert!(!contents.rectangular);
        // Limits apply in both directions; NUL is refused before anything changes.
        assert!(clipboard.text_within(4).is_err());
        assert!(clipboard.set_text("x".repeat(9).as_str(), 8).is_err());
        assert!(clipboard.set_text("nul\0", 64).is_err());
        assert_eq!(clipboard.text_within(64).unwrap().as_deref(), Some("ab\ncd"));
        // Plain text replaces the metadata too.
        clipboard.set_text("plain", 64).unwrap();
        assert_eq!(clipboard.metadata(RECTANGLE_CLIPBOARD_FORMAT, 64).unwrap(), None);
        assert!(clipboard.metadata("CF_UNICODETEXT", 64).is_err());
        println!("ok private_pasteboard_round_trips_text_and_metadata");
    }

    fn panels_and_alerts_are_configured_from_plans(mtm: MainThreadMarker) {
        let dialogs = MacDialogs::new(mtm);
        let options = SaveDialogOptions::new(SaveFileKind::Html)
            .named_after("notes.txt")
            .in_directory(Some(std::env::temp_dir()));
        let panel = dialogs.save_panel(&save_panel_plan(&options));
        // SAFETY: reading properties of a configured, never-shown panel.
        unsafe {
            assert_eq!(panel.nameFieldStringValue().to_string(), "notes.html");
            #[allow(deprecated, reason = "reads back the list the panel was given")]
            let allowed: Vec<String> = panel
                .allowedFileTypes()
                .unwrap()
                .iter()
                .map(|kind| kind.to_string())
                .collect();
            assert_eq!(allowed, ["html", "htm"]);
            assert!(panel.allowsOtherFileTypes());
            assert!(panel.directoryURL().is_some());
        }
        let plan = InAppPrompt::save_document("notes.txt").plan();
        let alert = dialogs.alert(&plan);
        // SAFETY: reading the buttons of a configured, never-shown alert.
        unsafe {
            let buttons = alert.buttons();
            assert_eq!(buttons.len(), 3);
            assert_eq!(buttons.get(1).unwrap().keyEquivalent().to_string(), "d");
        }
        assert_eq!(plan.answer(FIRST_BUTTON_RETURN + 2), CANCEL_ID);
        println!("ok panels_and_alerts_are_configured_from_plans");
    }

    fn appearance_and_language_are_readable(mtm: MainThreadMarker) {
        let appearance = MacAppearance::new(mtm);
        let first = appearance.changed().expect("the first poll always answers");
        assert_eq!(appearance.current(), first);
        assert_eq!(appearance.changed(), None);
        if let Some(language) = system_ui_language() {
            assert!(!language.contains('_'), "{language}");
        }
        println!("ok appearance_and_language_are_readable");
    }

    pub fn run() {
        let mtm = MainThreadMarker::new().expect("a harness = false test runs on the main thread");
        // No run loop drains autoreleased objects here, so each check gets a pool.
        objc2::rc::autoreleasepool(|_| menu_bar_builds_dispatches_and_follows_state(mtm));
        objc2::rc::autoreleasepool(|_| private_pasteboard_round_trips_text_and_metadata(mtm));
        objc2::rc::autoreleasepool(|_| panels_and_alerts_are_configured_from_plans(mtm));
        objc2::rc::autoreleasepool(|_| appearance_and_language_are_readable(mtm));
    }
}

fn main() {
    #[cfg(target_os = "macos")]
    appkit::run();
    #[cfg(not(target_os = "macos"))]
    println!("appkit checks run on macOS only");
}
