// SPDX-License-Identifier: MPL-2.0
//! Explicit QA metadata export; normal launches never write an inventory.
use super::extensions::{WorkerCompletion, extension_worker};
use super::*;
use bareline_platform::executor::WorkKind;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    ffi::OsString,
    io::Write,
    sync::{Arc, mpsc},
};

static INVENTORY_STAGE_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
pub(super) struct Request {
    path: PathBuf,
    commit: String,
}
#[derive(Default)]
pub(super) struct InventoryRuntime {
    request: Option<Request>,
    deadline: Option<Instant>,
    pending: Option<mpsc::Receiver<Result<PublishedInventory, String>>>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
}

struct PublishedInventory {
    path: PathBuf,
    sha256: String,
}

struct StagedInventory {
    path: PathBuf,
    published: bool,
}

impl Drop for StagedInventory {
    fn drop(&mut self) {
        if !self.published {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

impl Drop for InventoryRuntime {
    fn drop(&mut self) {
        self.cancel.store(true, std::sync::atomic::Ordering::Release);
    }
}

fn executable_sha256() -> Result<String, String> {
    let path = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut digest = Sha256::new();
    let mut block = [0_u8; 64 * 1024];
    loop {
        let count = std::io::Read::read(&mut file, &mut block).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        digest.update(&block[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn inventory_json(request: &Request, ids: &BTreeSet<String>) -> Result<String, String> {
    Ok(format!(
        "{{\"schema_version\":2,\"commit\":\"{}\",\"binary_sha256\":\"{}\",\"commands\":[{}]}}\n",
        request.commit,
        executable_sha256()?,
        ids.iter().map(|id| format!("\"{id}\"")).collect::<Vec<_>>().join(",")
    ))
}

fn write_inventory(
    request: Request,
    ids: BTreeSet<String>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
) -> Result<PublishedInventory, String> {
    if cancel.load(std::sync::atomic::Ordering::Acquire) {
        return Err("Inventory export cancelled".into());
    }
    let json = inventory_json(&request, &ids)?;
    let sha256 = format!("{:x}", Sha256::digest(json.as_bytes()));
    let path = request.path.clone();
    write_inventory_with_before_publish(request, json, cancel, || {})?;
    Ok(PublishedInventory { path, sha256 })
}

fn write_inventory_with_before_publish(
    request: Request,
    json: String,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    before_publish: impl FnOnce(),
) -> Result<(), String> {
    if cancel.load(std::sync::atomic::Ordering::Acquire) {
        return Err("Inventory export cancelled".into());
    }
    bareline_platform::LocalFileSystem::validate_target(&bareline_platform_windows::WindowsFileSystem, &request.path)
        .map_err(|e| e.to_string())?;
    let parent = request.path.parent().ok_or("Inventory parent unavailable")?;
    let operation = INVENTORY_STAGE_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let stage = parent.join(format!(
        ".bareline-command-inventory-{}-{operation}.tmp",
        std::process::id()
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&stage)
        .map_err(|e| e.to_string())?;
    let mut ownership = StagedInventory {
        path: stage.clone(),
        published: false,
    };
    file.write_all(json.as_bytes()).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    drop(file);
    before_publish();
    if cancel.load(std::sync::atomic::Ordering::Acquire) {
        return Err("Inventory export cancelled".into());
    }
    bareline_platform::LocalFileSystem::rename_entry(
        &bareline_platform_windows::WindowsFileSystem,
        &stage,
        &request.path,
    )
    .map_err(|e| e.to_string())?;
    ownership.published = true;
    Ok(())
}

fn submit_inventory(
    executor: &bareline_platform::executor::BoundedExecutor,
    request: Request,
    ids: BTreeSet<String>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    notify: Arc<dyn Fn() + Send + Sync>,
) -> Result<mpsc::Receiver<Result<PublishedInventory, String>>, bareline_platform::executor::SubmitError> {
    let (tx, rx) = mpsc::sync_channel(1);
    let completion = WorkerCompletion::new(tx, notify, "Inventory worker stopped");
    executor.submit(
        WorkKind::Bulk,
        Box::new(move || completion.complete(write_inventory(request, ids, cancel))),
    )?;
    Ok(rx)
}
pub(super) fn parse(args: Vec<OsString>) -> Result<(Vec<OsString>, Option<Request>), Box<dyn std::error::Error>> {
    let mut remaining = Vec::new();
    let mut path = None;
    let mut commit = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            remaining.push(arg);
            remaining.extend(args);
            break;
        }
        if arg == "--export-command-inventory" || arg == "--inventory-commit" {
            if !cfg!(feature = "qa-inventory") {
                return Err("Command inventory requires the qa-inventory build feature".into());
            }
            let value = args.next().ok_or("Missing command inventory argument")?;
            if arg == "--export-command-inventory" {
                if path.replace(PathBuf::from(value)).is_some() {
                    return Err("Duplicate inventory path".into());
                }
            } else if commit
                .replace(value.into_string().map_err(|_| "Inventory commit must be UTF-8")?)
                .is_some()
            {
                return Err("Duplicate inventory commit".into());
            }
        } else {
            remaining.push(arg);
        }
    }
    let request = match (path, commit) {
        (None, None) => None,
        (Some(path), Some(commit))
            if path.is_absolute()
                && commit.len() == 40
                && commit.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) =>
        {
            Some(Request { path, commit })
        }
        _ => return Err("Inventory requires an absolute new output path and a full lowercase commit SHA".into()),
    };
    if request.is_some() {
        remaining.splice(0..0, [OsString::from("--no-session"), OsString::from("--new-instance")]);
    }
    Ok((remaining, request))
}
impl InventoryRuntime {
    pub(super) fn deadline(&self) -> Option<Instant> {
        self.deadline
    }
    pub(super) fn configure(&mut self, request: Option<Request>) {
        self.cancel.store(true, std::sync::atomic::Ordering::Release);
        self.cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.request = request;
        self.deadline = self.request.as_ref().map(|_| Instant::now() + Duration::from_secs(60));
    }
}
impl Shell {
    pub(super) fn inventory_pump(&mut self, el: &ActiveEventLoop) {
        if self.inventory.deadline.is_none() {
            return;
        }
        if self
            .inventory
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            eprintln!("Command inventory discovery timed out");
            self.inventory.cancel.store(true, std::sync::atomic::Ordering::Release);
            self.failed = true;
            self.inventory.deadline = None;
            el.exit();
            return;
        }
        if let Some(pending) = &self.inventory.pending {
            match pending.try_recv() {
                Ok(Ok(published)) => {
                    eprintln!("command inventory published={}", published.path.display());
                    eprintln!("command inventory sha256={}", published.sha256);
                    self.inventory.deadline = None;
                    el.exit();
                }
                Ok(Err(error)) => {
                    eprintln!("command inventory: {error}");
                    self.failed = true;
                    self.inventory.deadline = None;
                    el.exit();
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.inventory.cancel.store(true, std::sync::atomic::Ordering::Release);
                    self.failed = true;
                    self.inventory.deadline = None;
                    el.exit();
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            return;
        }
        if !self.first_frame {
            return;
        }
        match self.extensions.command_inventory_ready() {
            Ok(false) => return,
            Err(error) => {
                eprintln!("Command inventory incomplete: {error}");
                self.failed = true;
                self.inventory.deadline = None;
                el.exit();
                return;
            }
            Ok(true) => {}
        }
        self.sync_contributions();
        let mut ids = BTreeSet::new();
        let valid = self
            .app
            .commands
            .entries()
            .map(|command| command.id.0.to_string())
            .chain(
                self.app
                    .commands
                    .contributions
                    .entries()
                    .map(|command| command.identity.id.clone()),
            )
            .all(|id| {
                !id.is_empty()
                    && id.len() <= 160
                    && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
                    && ids.insert(id)
            });
        if !valid || ids.len() > 10_000 {
            eprintln!("Command inventory has duplicate or unsupported IDs");
            self.failed = true;
            self.inventory.deadline = None;
            el.exit();
            return;
        }
        // Every registered command must route to a handler (ARCH-06/ARCH-15): a
        // command added without wiring it into `command_route`/`dispatch` fails
        // the inventory export instead of silently falling through at runtime.
        let unrouted = unrouted_registered_commands(&self.app.commands);
        if !unrouted.is_empty() {
            eprintln!("Command inventory has unrouted command IDs: {unrouted:?}");
            self.failed = true;
            self.inventory.deadline = None;
            el.exit();
            return;
        }
        let Some(request) = self.inventory.request.take() else {
            return;
        };
        eprintln!("command inventory target: {}", request.path.display());
        let notify = self.notify.clone();
        match submit_inventory(extension_worker(), request, ids, self.inventory.cancel.clone(), notify) {
            Ok(rx) => self.inventory.pending = Some(rx),
            Err(error) => {
                eprintln!("Command inventory worker unavailable: {error:?}");
                self.failed = true;
                self.inventory.deadline = None;
                el.exit();
            }
        }
    }
}

#[cfg(test)]
mod worker_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn inventory_identifies_the_running_binary_and_sorted_commands() {
        let request = Request {
            path: PathBuf::from("unused.json"),
            commit: "a".repeat(40),
        };
        let ids = BTreeSet::from(["file.save".to_owned(), "file.open".to_owned()]);
        let document: serde_json::Value = serde_json::from_str(&inventory_json(&request, &ids).unwrap()).unwrap();
        assert_eq!(document["schema_version"], 2);
        assert_eq!(document["commit"], "a".repeat(40));
        assert_eq!(document["binary_sha256"].as_str().unwrap().len(), 64);
        assert_eq!(document["commands"], serde_json::json!(["file.open", "file.save"]));
    }

    #[test]
    fn rejected_inventory_never_creates_stage_and_wakes_once() {
        let executor = bareline_platform::executor::BoundedExecutor::new(0, 1, "inventory-reject-test");
        let root = std::env::temp_dir().join(format!("bareline-inventory-reject-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("inventory.json");
        let (wake_tx, wake_rx) = mpsc::sync_channel(2);
        let result = submit_inventory(
            &executor,
            Request {
                path: target.clone(),
                commit: "a".repeat(40),
            },
            BTreeSet::new(),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            Arc::new(move || {
                let _ = wake_tx.try_send(());
            }),
        );
        assert!(matches!(result, Err(bareline_platform::executor::SubmitError::Closed)));
        wake_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(wake_rx.try_recv().is_err(), "rejected export woke more than once");
        assert!(!target.exists());
        assert!(std::fs::read_dir(&root).unwrap().next().is_none());
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn unpublished_inventory_stage_is_removed_on_owner_drop() {
        let root = std::env::temp_dir().join(format!("bareline-inventory-stage-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let stage = root.join("stage.tmp");
        std::fs::write(&stage, b"partial").unwrap();
        drop(StagedInventory {
            path: stage.clone(),
            published: false,
        });
        assert!(!stage.exists());
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn dropping_inventory_runtime_signals_owned_export() {
        let runtime = InventoryRuntime::default();
        let cancel = runtime.cancel.clone();
        drop(runtime);
        assert!(cancel.load(std::sync::atomic::Ordering::Acquire));
    }

    #[test]
    fn cancellation_before_inventory_publication_removes_stage() {
        let root = std::env::temp_dir().join(format!("bareline-inventory-cancel-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("inventory.json");
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let signal = cancel.clone();
        let result = write_inventory_with_before_publish(
            Request {
                path: target.clone(),
                commit: "a".repeat(40),
            },
            "{}".into(),
            cancel,
            move || signal.store(true, std::sync::atomic::Ordering::Release),
        );
        assert!(matches!(result, Err(ref error) if error == "Inventory export cancelled"));
        assert!(!target.exists());
        assert!(std::fs::read_dir(&root).unwrap().next().is_none());
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn queued_inventory_cancel_touches_no_target_or_stage() {
        let executor = bareline_platform::executor::BoundedExecutor::new(1, 1, "inventory-queued-cancel-test");
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        executor
            .submit(
                WorkKind::Bulk,
                Box::new(move || {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                }),
            )
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        let root = std::env::temp_dir().join(format!(
            "bareline-inventory-queued-cancel-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let target = root.join("inventory.json");
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let receiver = submit_inventory(
            &executor,
            Request {
                path: target.clone(),
                commit: "a".repeat(40),
            },
            BTreeSet::new(),
            cancel.clone(),
            Arc::new(|| {}),
        )
        .unwrap();
        cancel.store(true, std::sync::atomic::Ordering::Release);
        release_tx.send(()).unwrap();

        let result = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(result, Err(ref error) if error == "Inventory export cancelled"));
        assert!(!target.exists());
        assert!(std::fs::read_dir(&root).unwrap().next().is_none());
        std::fs::remove_dir(root).unwrap();
    }
}

/// Registered commands whose contributed ID no handler claims. An empty result
/// means every command has a dispatch route. Internal commands are excluded:
/// they are driven by bespoke code (macro replay, dynamic-contribution invoke)
/// rather than the contributed-command dispatch chain.
pub(super) fn unrouted_registered_commands(commands: &bareline_commands::CommandRegistry) -> Vec<&'static str> {
    commands
        .entries()
        .filter_map(|spec| match spec.action {
            bareline_commands::Action::Contributed(id) => {
                let internal = commands
                    .presentation(id)
                    .is_some_and(|presentation| presentation.internal);
                if internal {
                    return None;
                }
                super::command_route(id.0).is_none().then_some(id.0)
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod route_tests {
    use super::*;
    use bareline_commands::{Action, CommandId, CommandSpec};

    fn production_registry() -> bareline_commands::CommandRegistry {
        let mut app = bareline_app::App::default();
        super::super::register_all_commands(&mut app.commands);
        app.commands
    }

    /// PR-011: assembling the production registry registers every command ID
    /// once. Every composition-root registrar fails loudly on a duplicate in
    /// debug and tests, so building the registry without a panic proves there
    /// is none, and re-running each registrar proves none of them drops one.
    #[test]
    fn composition_root_rejects_duplicate_command_ids() {
        let commands = production_registry();
        assert!(commands.entries().count() > 0);
        if !cfg!(debug_assertions) {
            return;
        }
        let registrars: [(&str, fn(&mut bareline_commands::CommandRegistry)); 15] = [
            ("language", bareline_app::language::register_commands),
            ("extensions", super::super::extensions::register),
            ("toolbar", super::super::toolbar::register),
            ("shortcuts", super::super::shortcuts::register),
            ("goto", super::super::goto::register),
            ("lifecycle", super::super::lifecycle::register),
            ("power", super::super::power::register),
            ("utilities", super::super::utilities::register),
            ("search", super::super::search::register),
            ("encoding", bareline_app::encoding::register),
            ("compare", super::super::compare::register),
            ("dock", super::super::dock::register),
            ("views", super::super::views::register),
            ("macros", bareline_app::macros::register_commands),
            ("workspace panel", bareline_app::workspace_panel::register_commands),
        ];
        for (name, register) in registrars {
            let duplicate = std::panic::catch_unwind(move || {
                let mut commands = production_registry();
                register(&mut commands);
            });
            assert!(
                duplicate.is_err(),
                "{name} silently ignored a duplicate command registration"
            );
        }
    }

    #[test]
    fn every_registered_command_routes_to_a_handler() {
        let commands = production_registry();
        let unrouted = unrouted_registered_commands(&commands);
        assert!(
            unrouted.is_empty(),
            "these registered commands have no dispatch handler: {unrouted:?}"
        );
    }

    #[test]
    fn a_registered_command_without_a_handler_fails_validation() {
        for id in ["search.mode.literal", "search.mode.extended", "search.mode.regex"] {
            assert_eq!(super::super::command_route(id), Some(super::super::Route::Search));
        }
        assert_eq!(super::super::command_route("search.mode.unknown"), None);
        let mut commands = production_registry();
        // A brand-new command with no route must be reported by the same check
        // the inventory export runs at startup.
        commands
            .register(CommandSpec {
                id: CommandId("qa.probe.without.handler"),
                title: "QA Probe Without Handler",
                category: "Tools",
                shortcut: "",
                action: Action::Contributed(CommandId("qa.probe.without.handler")),
            })
            .expect("probe registers");
        assert_eq!(
            unrouted_registered_commands(&commands),
            vec!["qa.probe.without.handler"]
        );
    }

    #[test]
    fn every_menu_and_context_menu_id_is_registered() {
        let commands = production_registry();
        let context_menus: [&[&str]; 5] = [
            &super::super::TAB_CONTEXT_COMMANDS,
            &super::super::EDITOR_CONTEXT_COMMANDS,
            &super::super::FALLBACK_CONTEXT_COMMANDS,
            &super::super::workspace_panels::DOCUMENTS_CONTEXT_COMMANDS,
            &super::super::workspace_panels::EXPLORER_CONTEXT_COMMANDS,
        ];
        let listed = bareline_app::menus::tree_command_ids()
            .into_iter()
            .chain(context_menus.into_iter().flatten().copied())
            .chain(bareline_app::menus::WHEN_ENABLED.iter().copied())
            .chain(bareline_app::menus::PALETTE_ONLY.iter().copied())
            .filter(|id| *id != "-");
        let missing: Vec<&str> = listed.filter(|id| commands.lookup(id).is_none()).collect();
        assert!(
            missing.is_empty(),
            "menus name unregistered commands (they would be dropped silently): {missing:?}"
        );
        // The tab menu offers the multi-close, path and rename actions (WSP-01).
        for id in [
            "view.tabs.closeAll",
            "view.tabs.closeOthers",
            "view.tabs.closeLeft",
            "view.tabs.closeRight",
            "file.copyPath",
            "file.copyName",
            "file.copyDirectory",
            "file.rename",
        ] {
            assert!(super::super::TAB_CONTEXT_COMMANDS.contains(&id), "{id}");
        }
    }

    #[test]
    fn every_command_has_a_menu_home_and_a_human_title() {
        let commands = production_registry();
        let model = bareline_app::menus::curated_model(&commands);
        let mut homeless = Vec::new();
        for item in &model.items {
            let bareline_commands::MenuItem::Submenu { title, items } = item else {
                continue;
            };
            assert_ne!(title, bareline_commands::OTHER_MENU, "no top-level catch-all");
            if title != "Tools" {
                continue;
            }
            for item in items {
                if let bareline_commands::MenuItem::Submenu { title, items } = item
                    && title == bareline_commands::OTHER_MENU
                {
                    homeless.extend(items.iter().filter_map(|item| match item {
                        bareline_commands::MenuItem::Command(id) => Some(id.0),
                        _ => None,
                    }));
                }
            }
        }
        assert!(
            homeless.is_empty(),
            "commands fell into Tools > Other; give them a place in bareline_app::menus::TREE: {homeless:?}"
        );
        for spec in commands.entries() {
            assert!(
                !bareline_commands::title_looks_like_identifier(spec.title),
                "{} has an identifier-like title {:?}",
                spec.id.0,
                spec.title
            );
        }
    }

    #[test]
    fn unavailable_update_and_extension_features_stay_out_of_menus() {
        let commands = production_registry();
        let internal = |id: &'static str| {
            commands
                .presentation(CommandId(id))
                .is_some_and(|presentation| presentation.internal)
        };
        for id in [
            "update.check",
            "update.apply_on_exit",
            "update.cancel",
            "update.discard",
        ] {
            assert_eq!(internal(id), !super::super::update::available(), "{id}");
        }
        for id in ["extensions.manage", "ext.json.format", "ext.hex.open"] {
            assert_eq!(internal(id), !super::super::extensions::trust_available(), "{id}");
        }
    }

    /// `us_key_press` spells keys the way `settings::key_press` names winit's
    /// physical and logical keys.
    #[test]
    fn us_key_press_uses_the_names_key_press_reads_from_winit() {
        use super::super::settings::{key_name, physical_key_name};
        use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};
        for (code, name) in [
            (KeyCode::KeyA, "KeyA"),
            (KeyCode::Digit1, "Digit1"),
            (KeyCode::Slash, "Slash"),
            (KeyCode::Minus, "Minus"),
            (KeyCode::Equal, "Equal"),
            (KeyCode::BracketLeft, "BracketLeft"),
            (KeyCode::BracketRight, "BracketRight"),
            (KeyCode::Backslash, "Backslash"),
            (KeyCode::Semicolon, "Semicolon"),
            (KeyCode::Quote, "Quote"),
            (KeyCode::Comma, "Comma"),
            (KeyCode::Period, "Period"),
            (KeyCode::Backquote, "Backquote"),
            (KeyCode::ArrowUp, "ArrowUp"),
            (KeyCode::PageDown, "PageDown"),
            (KeyCode::F5, "F5"),
        ] {
            assert_eq!(physical_key_name(PhysicalKey::Code(code)).as_deref(), Some(name));
        }
        for (key, name) in [
            (NamedKey::ArrowUp, "ArrowUp"),
            (NamedKey::ArrowDown, "ArrowDown"),
            (NamedKey::PageUp, "PageUp"),
            (NamedKey::Escape, "Escape"),
            (NamedKey::Tab, "Tab"),
            (NamedKey::Space, "Space"),
            (NamedKey::Enter, "Enter"),
            (NamedKey::Delete, "Delete"),
            (NamedKey::Backspace, "Backspace"),
            (NamedKey::Insert, "Insert"),
            (NamedKey::Home, "Home"),
            (NamedKey::End, "End"),
            (NamedKey::F5, "F5"),
        ] {
            assert_eq!(key_name(&Key::Named(key)).as_deref(), Some(name));
        }
        assert_eq!(key_name(&Key::Character("?".into())).as_deref(), Some("?"));
    }

    /// The key press a US keyboard produces for `chord`, as winit reports it.
    fn us_key_press(chord: &bareline_commands::KeyChord) -> bareline_commands::KeyPress {
        use bareline_commands::{Key, KeyPress};
        const SHIFTED: [(char, char, &str); 11] = [
            ('/', '?', "Slash"),
            ('-', '_', "Minus"),
            ('=', '+', "Equal"),
            ('[', '{', "BracketLeft"),
            (']', '}', "BracketRight"),
            ('\\', '|', "Backslash"),
            (';', ':', "Semicolon"),
            ('\'', '"', "Quote"),
            (',', '<', "Comma"),
            ('.', '>', "Period"),
            ('`', '~', "Backquote"),
        ];
        const DIGITS_SHIFTED: [char; 10] = [')', '!', '@', '#', '$', '%', '^', '&', '*', '('];
        let mut press = KeyPress {
            ctrl: chord.ctrl,
            alt: chord.alt,
            shift: chord.shift,
            meta: chord.meta,
            ..Default::default()
        };
        match &chord.key {
            Key::Physical(code) => press.physical = Some(code.clone()),
            Key::Logical(key) if key.chars().count() == 1 => {
                let base = key.chars().next().unwrap().to_ascii_lowercase();
                let (shifted, code) = if base.is_ascii_alphabetic() {
                    (base.to_ascii_uppercase(), format!("Key{}", base.to_ascii_uppercase()))
                } else if let Some(digit) = base.to_digit(10) {
                    (DIGITS_SHIFTED[digit as usize], format!("Digit{base}"))
                } else {
                    let (_, shifted, code) = SHIFTED
                        .iter()
                        .find(|(plain, _, _)| *plain == base)
                        .unwrap_or_else(|| panic!("no US key for {base:?}"));
                    (*shifted, (*code).to_owned())
                };
                press.physical = Some(code);
                press.unmodified = Some(base.to_string());
                press.logical = Some(if chord.shift { shifted } else { base }.to_string());
            }
            Key::Logical(named) => {
                // winit names these keys the same way physically and logically.
                let name = match named.as_str() {
                    "UP" => "ArrowUp",
                    "DOWN" => "ArrowDown",
                    "LEFT" => "ArrowLeft",
                    "RIGHT" => "ArrowRight",
                    "PAGEUP" => "PageUp",
                    "PAGEDOWN" => "PageDown",
                    "ESCAPE" => "Escape",
                    "TAB" => "Tab",
                    "SPACE" => "Space",
                    "ENTER" => "Enter",
                    "DELETE" => "Delete",
                    "BACKSPACE" => "Backspace",
                    "INSERT" => "Insert",
                    "HOME" => "Home",
                    "END" => "End",
                    other => other,
                };
                press.physical = Some(name.to_owned());
                press.logical = Some(name.to_owned());
                press.unmodified = Some(name.to_owned());
            }
        }
        press
    }

    #[test]
    fn every_default_binding_resolves_to_its_command() {
        use bareline_commands::{InputContext, KeyResolution, Keymap};
        let commands = production_registry();
        let keymap = Keymap::defaults(&commands);
        assert!(!keymap.bindings().is_empty());
        for binding in keymap.bindings() {
            let [chord] = binding.sequence.as_slice() else {
                panic!("default bindings are single chords: {}", binding.command.0);
            };
            let press = us_key_press(chord);
            // Windows reports AltGr as Ctrl+Alt, and the shell passes it that way.
            let context = InputContext {
                alt_gr: press.ctrl && press.alt,
                ..Default::default()
            };
            let resolved =
                press
                    .candidates()
                    .into_iter()
                    .find_map(|candidate| match keymap.resolve(&[candidate], context) {
                        KeyResolution::Command(id) => Some(id),
                        _ => None,
                    });
            assert_eq!(
                resolved,
                Some(binding.command),
                "{} is bound to {} but the key press does not reach it",
                binding.command.0,
                chord.label()
            );
        }
    }

    #[test]
    fn a_duplicate_command_id_is_rejected_at_registration() {
        let mut commands = production_registry();
        let duplicate = commands.register(CommandSpec {
            id: CommandId("recovery.open"),
            title: "Duplicate Recovery Center",
            category: "File",
            shortcut: "",
            action: Action::Contributed(CommandId("recovery.open")),
        });
        assert_eq!(duplicate, Err(CommandId("recovery.open")));
    }
}

#[cfg(all(test, feature = "qa-inventory"))]
mod tests {
    use super::*;
    #[test]
    fn inventory_arguments_require_provenance_and_preserve_option_terminator() {
        let hash = "a".repeat(40);
        let args = vec![
            "--export-command-inventory".into(),
            "C:\\qa\\commands.json".into(),
            "--inventory-commit".into(),
            hash.into(),
            "--".into(),
            "-literal.txt".into(),
        ];
        let (rest, request) = parse(args).unwrap();
        assert!(request.is_some());
        assert_eq!(
            rest,
            vec![
                OsString::from("--no-session"),
                OsString::from("--new-instance"),
                OsString::from("--"),
                OsString::from("-literal.txt")
            ]
        );
        assert!(parse(vec!["--inventory-commit".into(), "bad".into()]).is_err());
        assert!(
            parse(vec![
                "--export-command-inventory".into(),
                "relative.json".into(),
                "--inventory-commit".into(),
                "b".repeat(40).into()
            ])
            .is_err()
        );
    }
}
