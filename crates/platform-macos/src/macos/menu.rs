// SPDX-License-Identifier: MPL-2.0
//! The native menu bar: `NSMenu` and `NSMenuItem` built from a [`MenuPlan`].
//!
//! Main thread only. [`MacMenuBar::new`] takes a `MainThreadMarker`, and the
//! menu bar holds AppKit objects that are not `Send`, so every later call is
//! made on the main thread too. The shell calls it from winit's event loop,
//! which runs on the main thread on macOS.
//!
//! Commands. Items carry their plan tag. A chosen item sends
//! `barelineMenuCommand:` to Bareline's target, which pushes a
//! [`CommandMessage`] into the channel the seam polls, then calls `notify` to
//! wake the event loop, as the Windows message hook does for `WM_COMMAND`.
//! Undo, Redo, Cut, Copy, Paste and Select All send the standard actions to
//! the first responder instead, so panel and alert text fields handle them;
//! the target answers them in the editor window once
//! [`MacMenuBar::attach_to_view`] has put it in the responder chain.
//!
//! State. Items validate through the target (`validateMenuItem:`), so an
//! item is enabled exactly when its command state says so, and AppKit's modal
//! rule applies: while a panel or alert runs, Bareline's items are disabled
//! and their key equivalents reach the panel.
use super::ns_string;
use crate::{
    keys::KeyEquivalent,
    menu_plan::{CommandMessage, ItemAction, MenuPlan, MenuProjection, MenuRole, PlanItem, PlanMenu, PlanNode},
};
use bareline_commands::{Action, CommandContext, CommandId, CommandRegistry, Keymap, MenuModel};
use objc2::{
    ClassType, DeclaredClass, declare_class, msg_send_id, mutability,
    rc::Retained,
    runtime::{AnyObject, NSObjectProtocol, Sel},
    sel,
};
use objc2_app_kit::{
    NSApplication, NSControlStateValueOff, NSControlStateValueOn, NSEventModifierFlags, NSMenu, NSMenuItem,
    NSResponder, NSView,
};
use objc2_foundation::MainThreadMarker;
use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    ptr::NonNull,
    sync::{Arc, mpsc::Sender},
};

/// Right-click actions per command: (action code, label) pairs.
type ItemActionSource = Vec<(CommandId, Vec<(u16, String)>)>;

/// Where chosen commands go: the channel the seam polls and the event-loop wake.
#[derive(Clone)]
pub struct MenuDispatch {
    pub sender: Sender<CommandMessage>,
    pub notify: Arc<dyn Fn() + Send + Sync>,
}

pub(crate) struct TargetIvars {
    dispatch: MenuDispatch,
    window: isize,
    /// Enabled state by tag - 1.
    enabled: RefCell<Vec<bool>>,
    /// The tag each standard responder action stands for in the current plan.
    responder_tags: RefCell<Vec<(Sel, isize)>>,
}

declare_class!(
    /// Bareline's menu target, and its responder in the editor window.
    pub(crate) struct MenuTarget;

    unsafe impl ClassType for MenuTarget {
        type Super = NSResponder;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "BarelineMenuTarget";
    }

    impl DeclaredClass for MenuTarget {
        type Ivars = TargetIvars;
    }

    unsafe impl MenuTarget {
        #[method(barelineMenuCommand:)]
        fn menu_command(&self, sender: &NSMenuItem) {
            // SAFETY: reading the tag of a live menu item on the main thread.
            self.dispatch(unsafe { sender.tag() });
        }

        #[method(validateMenuItem:)]
        fn validate_menu_item(&self, item: &NSMenuItem) -> bool {
            // SAFETY: as above.
            let tag = unsafe { item.tag() };
            self.enabled(tag)
        }

        #[method(undo:)]
        fn undo(&self, _sender: Option<&AnyObject>) {
            self.responder_action(sel!(undo:));
        }

        #[method(redo:)]
        fn redo(&self, _sender: Option<&AnyObject>) {
            self.responder_action(sel!(redo:));
        }

        #[method(cut:)]
        fn cut(&self, _sender: Option<&AnyObject>) {
            self.responder_action(sel!(cut:));
        }

        #[method(copy:)]
        fn copy(&self, _sender: Option<&AnyObject>) {
            self.responder_action(sel!(copy:));
        }

        #[method(paste:)]
        fn paste(&self, _sender: Option<&AnyObject>) {
            self.responder_action(sel!(paste:));
        }

        #[method(selectAll:)]
        fn select_all(&self, _sender: Option<&AnyObject>) {
            self.responder_action(sel!(selectAll:));
        }
    }

    unsafe impl NSObjectProtocol for MenuTarget {}
);

impl MenuTarget {
    fn new(mtm: MainThreadMarker, ivars: TargetIvars) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(ivars);
        // SAFETY: NSResponder's designated initializer, on a fresh allocation.
        unsafe { msg_send_id![super(this), init] }
    }
    fn enabled(&self, tag: isize) -> bool {
        usize::try_from(tag)
            .ok()
            .and_then(|tag| tag.checked_sub(1))
            .and_then(|index| self.ivars().enabled.borrow().get(index).copied())
            .unwrap_or(false)
    }
    fn dispatch(&self, tag: isize) {
        let Ok(id) = usize::try_from(tag) else {
            return;
        };
        if !self.enabled(tag) {
            return;
        }
        let ivars = self.ivars();
        let message = CommandMessage {
            hwnd: ivars.window,
            id,
            action: 0,
        };
        // A closed channel means the shell is shutting down; nothing to wake.
        if ivars.dispatch.sender.send(message).is_ok() {
            (ivars.dispatch.notify)();
        }
    }
    fn responder_action(&self, action: Sel) {
        let tag = self
            .ivars()
            .responder_tags
            .borrow()
            .iter()
            .find(|(selector, _)| *selector == action)
            .map(|(_, tag)| *tag);
        if let Some(tag) = tag {
            self.dispatch(tag);
        }
    }
}

/// Everything a menu projection is derived from besides the plan itself.
struct SyncKey {
    commands: usize,
    context: CommandContext,
    keymap: Keymap,
    locale_revision: u64,
}

/// The native objects of one built plan, in [`MenuPlan::walk`] order.
struct Built {
    root: Retained<NSMenu>,
    /// By tag - 1; `None` for a registered command whose menu was left out.
    items: Vec<Option<Retained<NSMenuItem>>>,
    /// Each menu with the item that opens it.
    menus: Vec<(Retained<NSMenu>, Retained<NSMenuItem>)>,
    system: Vec<Retained<NSMenuItem>>,
}

pub struct MacMenuBar {
    mtm: MainThreadMarker,
    window: isize,
    target: Retained<MenuTarget>,
    /// The full curated menu; the visible subset is rebuilt from it on demand.
    model: MenuModel,
    plan: MenuPlan,
    built: Built,
    /// Command order of the visible menu as built, so structural rebuilds only
    /// happen when a contextual command appears or disappears.
    order: Vec<CommandId>,
    applied: RefCell<Option<MenuProjection>>,
    synced: RefCell<Option<SyncKey>>,
    structure_checked: Option<(usize, CommandContext)>,
    /// The editor view the target is chained behind, if attached.
    view: RefCell<Option<Retained<NSView>>>,
    attached: Cell<bool>,
    item_actions: RefCell<ItemActionSource>,
}

fn set_key(item: &NSMenuItem, key: Option<&KeyEquivalent>) {
    let (text, mask) = key.map_or(("", 0), |key| (key.key.as_str(), key.modifiers.0));
    // SAFETY: plain property setters on a live item, on the main thread.
    unsafe { item.setKeyEquivalent(&ns_string(text)) };
    item.setKeyEquivalentModifierMask(NSEventModifierFlags(mask as usize));
}

impl MacMenuBar {
    /// Builds the menu bar for `model` and installs it as the application's
    /// main menu. `window` is the seam's token for the editor window; it comes
    /// back in every [`CommandMessage`]. The first projection uses the
    /// registry's default keymap and an empty context; the shell's first frame
    /// syncs the real ones.
    pub fn new(
        mtm: MainThreadMarker,
        window: isize,
        registry: &CommandRegistry,
        model: MenuModel,
        dispatch: MenuDispatch,
    ) -> Self {
        let target = MenuTarget::new(
            mtm,
            TargetIvars {
                dispatch,
                window,
                enabled: RefCell::new(Vec::new()),
                responder_tags: RefCell::new(Vec::new()),
            },
        );
        let context = CommandContext::default();
        let visible = model.visible(registry, &context);
        let plan = MenuPlan::build(&visible, registry);
        let built = Self::build(mtm, &plan, &target);
        let bar = Self {
            mtm,
            window,
            target,
            model,
            plan,
            built,
            order: visible.command_order(),
            applied: RefCell::new(None),
            synced: RefCell::new(None),
            structure_checked: None,
            view: RefCell::new(None),
            attached: Cell::new(false),
            item_actions: RefCell::new(Vec::new()),
        };
        bar.install();
        bar.sync_commands_localized(
            registry,
            &context,
            &Keymap::defaults(registry),
            u64::MAX,
            |_, fallback| fallback.to_owned(),
        );
        bar
    }

    fn build(mtm: MainThreadMarker, plan: &MenuPlan, target: &MenuTarget) -> Built {
        let mut built = Built {
            // SAFETY: a fresh, empty menu.
            root: unsafe { NSMenu::initWithTitle(mtm.alloc(), &ns_string("")) },
            items: vec![None; plan.len()],
            menus: Vec::new(),
            system: Vec::new(),
        };
        let mut responder_tags = Vec::new();
        for menu in &plan.menus {
            let item = Self::submenu(mtm, menu, target, &mut built, &mut responder_tags);
            built.root.addItem(&item);
        }
        *target.ivars().responder_tags.borrow_mut() = responder_tags;
        *target.ivars().enabled.borrow_mut() = vec![false; plan.len()];
        built
    }

    /// The item that opens `menu`, with the menu and its items built; pushes
    /// in [`MenuPlan::walk`] order.
    fn submenu(
        mtm: MainThreadMarker,
        menu: &PlanMenu,
        target: &MenuTarget,
        built: &mut Built,
        responder_tags: &mut Vec<(Sel, isize)>,
    ) -> Retained<NSMenuItem> {
        let title = ns_string(&menu.title);
        // SAFETY: a fresh menu; the title is copied.
        let native = unsafe { NSMenu::initWithTitle(mtm.alloc(), &title) };
        // SAFETY: a fresh item with no action; the title is copied.
        let opener =
            unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(mtm.alloc(), &title, None, &ns_string("")) };
        opener.setSubmenu(Some(&native));
        built.menus.push((native.clone(), opener.clone()));
        let app = NSApplication::sharedApplication(mtm);
        // SAFETY: the menus are live and owned by the menu bar; AppKit retains them.
        unsafe {
            match menu.role {
                MenuRole::Services => app.setServicesMenu(Some(&native)),
                MenuRole::Help => app.setHelpMenu(Some(&native)),
                MenuRole::Application | MenuRole::Ordinary => {}
            }
        }
        for entry in &menu.items {
            let item = match entry {
                PlanItem::Separator => NSMenuItem::separatorItem(mtm),
                PlanItem::Submenu(child) => Self::submenu(mtm, child, target, built, responder_tags),
                PlanItem::System(system) => {
                    let selector = Sel::register(system.selector);
                    // SAFETY: an untargeted standard action, which NSApplication implements.
                    let item = unsafe {
                        NSMenuItem::initWithTitle_action_keyEquivalent(
                            mtm.alloc(),
                            &ns_string(system.title),
                            Some(selector),
                            &ns_string(""),
                        )
                    };
                    set_key(&item, system.key.as_ref());
                    built.system.push(item.clone());
                    item
                }
                PlanItem::Command(command) => {
                    let (selector, targeted) = match command.action {
                        ItemAction::Dispatch => (sel!(barelineMenuCommand:), true),
                        ItemAction::Responder(name) => {
                            let selector = Sel::register(name);
                            responder_tags.push((selector, command.tag));
                            (selector, false)
                        }
                    };
                    // SAFETY: the target implements the selector; the title is
                    // replaced by the first projection.
                    let item = unsafe {
                        let item = NSMenuItem::initWithTitle_action_keyEquivalent(
                            mtm.alloc(),
                            &ns_string(command.command.0),
                            Some(selector),
                            &ns_string(""),
                        );
                        item.setTag(command.tag);
                        if targeted {
                            item.setTarget(Some(target));
                        }
                        item
                    };
                    if let Some(slot) = usize::try_from(command.tag - 1)
                        .ok()
                        .and_then(|index| built.items.get_mut(index))
                    {
                        *slot = Some(item.clone());
                    }
                    item
                }
            };
            native.addItem(&item);
        }
        opener
    }

    fn install(&self) {
        NSApplication::sharedApplication(self.mtm).setMainMenu(Some(&self.built.root));
    }

    /// The main menu this bar installed.
    pub fn main_menu(&self) -> &NSMenu {
        &self.built.root
    }
    pub fn plan(&self) -> &MenuPlan {
        &self.plan
    }
    pub fn command_id(&self, menu_id: usize) -> Option<CommandId> {
        self.plan.command_id(menu_id)
    }
    pub fn action(&self, menu_id: usize) -> Option<Action> {
        self.plan.action(menu_id)
    }
    pub fn accepts_command(&self, message: &CommandMessage) -> bool {
        message.hwnd == self.window
    }

    /// Updates titles, states and key equivalents from the same state and
    /// keymap as the palette. A sync whose inputs all match the last one
    /// returns without touching the menu; `locale_revision` must change
    /// whenever `label_for` would answer differently.
    pub fn sync_commands_localized(
        &self,
        registry: &CommandRegistry,
        context: &CommandContext,
        keymap: &Keymap,
        locale_revision: u64,
        label_for: impl Fn(&str, &str) -> String,
    ) {
        let commands = registry.entries().count();
        if self.synced.borrow().as_ref().is_some_and(|key| {
            key.commands == commands
                && key.locale_revision == locale_revision
                && key.context == *context
                && key.keymap == *keymap
        }) {
            return;
        }
        let projection = self.plan.project(registry, context, keymap, label_for);
        self.apply(projection);
        *self.synced.borrow_mut() = Some(SyncKey {
            commands,
            context: context.clone(),
            keymap: keymap.clone(),
            locale_revision,
        });
    }

    fn apply(&self, projection: MenuProjection) {
        if self.applied.borrow().as_ref() == Some(&projection) {
            return;
        }
        {
            let mut enabled = self.target.ivars().enabled.borrow_mut();
            for state in &projection.items {
                let Some(index) = usize::try_from(state.tag - 1).ok() else {
                    continue;
                };
                if let Some(flag) = enabled.get_mut(index) {
                    *flag = state.enabled;
                }
                let Some(Some(item)) = self.built.items.get(index) else {
                    continue;
                };
                // SAFETY: property setters on a live item, on the main thread.
                unsafe {
                    item.setTitle(&ns_string(&state.title));
                    item.setState(if state.checked {
                        NSControlStateValueOn
                    } else {
                        NSControlStateValueOff
                    });
                }
                set_key(item, state.key.as_ref());
            }
        }
        for ((menu, opener), title) in self.built.menus.iter().zip(&projection.menus) {
            let title = ns_string(title);
            // SAFETY: as above.
            unsafe {
                menu.setTitle(&title);
                opener.setTitle(&title);
            }
        }
        for (item, title) in self.built.system.iter().zip(&projection.system) {
            // SAFETY: as above.
            unsafe { item.setTitle(&ns_string(title)) };
        }
        *self.applied.borrow_mut() = Some(projection);
    }

    /// Rebuilds the menus only when the set or order of visible commands
    /// changed (a document finished loading, the Window list grew). Cheap to
    /// call every frame: it compares a fingerprint and usually returns at once.
    /// The next sync projects titles and states onto the new items.
    pub fn refresh_structure(&mut self, registry: &CommandRegistry, context: &CommandContext) {
        let commands = registry.entries().count();
        if self
            .structure_checked
            .as_ref()
            .is_some_and(|(count, checked)| *count == commands && checked == context)
        {
            return;
        }
        let visible = self.model.visible(registry, context);
        let order = visible.command_order();
        if order != self.order {
            self.plan = MenuPlan::build(&visible, registry);
            self.built = Self::build(self.mtm, &self.plan, &self.target);
            self.order = order;
            self.applied.borrow_mut().take();
            self.synced.borrow_mut().take();
            self.install();
        }
        self.structure_checked = Some((commands, context.clone()));
    }

    /// Right-click actions on menu items (Pin and Remove on a Recent Files
    /// entry) have no AppKit equivalent; they are kept so the shell's call is
    /// uniform, and stay reachable from the shell's own UI.
    pub fn set_menu_item_actions(&self, actions: &[(CommandId, Vec<(u16, String)>)]) {
        if self.item_actions.borrow().as_slice() != actions {
            *self.item_actions.borrow_mut() = actions.to_vec();
        }
    }

    /// Puts the menu target into the editor view's responder chain, right
    /// after the view, so the standard edit actions reach Bareline when the
    /// editor window is key. Calling it again for the same view does nothing.
    ///
    /// # Safety
    /// `ns_view` is the live `NSView` of the editor window (winit's
    /// `AppKitWindowHandle::ns_view`), used on the main thread.
    pub unsafe fn attach_to_view(&self, ns_view: NonNull<c_void>) {
        // SAFETY: the caller guarantees a live NSView; retaining keeps it alive
        // until the target is detached.
        let Some(view) = (unsafe { Retained::retain(ns_view.as_ptr().cast::<NSView>()) }) else {
            return;
        };
        if self
            .view
            .borrow()
            .as_ref()
            .is_some_and(|attached| Retained::as_ptr(attached) == Retained::as_ptr(&view))
        {
            return;
        }
        self.detach();
        // SAFETY: responder links between live objects on the main thread.
        unsafe {
            let next = view.nextResponder();
            self.target.setNextResponder(next.as_deref());
            view.setNextResponder(Some(&self.target));
        }
        *self.view.borrow_mut() = Some(view);
        self.attached.set(true);
    }

    fn detach(&self) {
        let Some(view) = self.view.borrow_mut().take() else {
            return;
        };
        self.attached.set(false);
        // SAFETY: as in `attach_to_view`. The chain is restored only if nothing
        // else was inserted after the view meanwhile.
        unsafe {
            let ours = view
                .nextResponder()
                .is_some_and(|next| std::ptr::eq(&*next, &**self.target as &NSResponder));
            if ours {
                let next = self.target.nextResponder();
                view.setNextResponder(next.as_deref());
            }
            self.target.setNextResponder(None);
        }
    }

    /// Whether the target is in an editor view's responder chain.
    pub fn attached(&self) -> bool {
        self.attached.get()
    }

    /// The tags of the items the plan walk visits, for diagnostics and tests.
    pub fn item_titles(&self) -> Vec<(isize, String)> {
        let mut titles = Vec::new();
        self.plan.walk(&mut |node| {
            if let PlanNode::Command(command) = node
                && let Some(Some(item)) = usize::try_from(command.tag - 1)
                    .ok()
                    .and_then(|index| self.built.items.get(index))
            {
                // SAFETY: reading a live item's title on the main thread.
                titles.push((command.tag, unsafe { item.title() }.to_string()));
            }
        });
        titles
    }
}

impl Drop for MacMenuBar {
    fn drop(&mut self) {
        self.detach();
        let app = NSApplication::sharedApplication(self.mtm);
        // SAFETY: reading the main menu on the main thread.
        let installed = unsafe { app.mainMenu() };
        if installed.is_some_and(|menu| Retained::as_ptr(&menu) == Retained::as_ptr(&self.built.root)) {
            app.setMainMenu(None);
        }
    }
}
