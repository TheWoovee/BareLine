// SPDX-License-Identifier: MPL-2.0
//! Typed, event-driven shell notifications with retained operation ownership.
use bareline_renderer::{DrawOp, Point, Rect, TextBackend};
use bareline_ui::theme::{ToastLevel, UiTheme};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use winit::keyboard::{Key, NamedKey};

const INFO_LIFETIME: Duration = Duration::from_secs(6);
const WIDTH: f32 = 380.0;
const ROW_HEIGHT: f32 = 58.0;
const GAP: f32 = 8.0;
const MAX_VISIBLE: usize = 6;
const MAX_TRANSIENT: usize = 6;
const MAX_RESOLVED_HISTORY: usize = 64;
const OVERFLOW_ID: u64 = 90_099_990;
pub(super) const DETAILS_GROUP_ID: u64 = 90_099_980;
pub(super) const DETAILS_CONTENT_ID: u64 = 90_099_981;
pub(super) const DETAILS_CLOSE_ID: u64 = 90_099_982;

pub(super) fn next_revision() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

pub(super) type DocumentKey = (u64, u64);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct NotificationId(pub(super) String);
impl From<&str> for NotificationId {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}
impl From<String> for NotificationId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NotificationKind {
    Progress,
    Outcome,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NotificationLifetime {
    Scoped,
    Transient,
    Persistent,
}

#[derive(Clone, Debug)]
pub(super) struct Notification {
    pub(super) id: NotificationId,
    pub(super) revision: u64,
    pub(super) level: ToastLevel,
    pub(super) kind: NotificationKind,
    pub(super) text: String,
    pub(super) details: Option<String>,
    pub(super) document: Option<DocumentKey>,
    pub(super) lifetime: NotificationLifetime,
}

impl Notification {
    pub(super) fn new(
        id: impl Into<NotificationId>,
        revision: u64,
        level: ToastLevel,
        kind: NotificationKind,
        text: impl Into<String>,
        details: Option<String>,
        document: Option<DocumentKey>,
        lifetime: NotificationLifetime,
    ) -> Self {
        Self {
            id: id.into(),
            revision,
            level,
            kind,
            text: text.into(),
            details,
            document,
            lifetime,
        }
    }
}

struct Toast {
    notification: Notification,
    accessibility_id: u64,
    born: Instant,
    bounds: Rect,
}

#[derive(Clone, Copy)]
enum Hit {
    Details(u64),
    Dismiss(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ToastAction {
    Consumed,
    OpenDetails(u64),
    Dismiss(u64),
}

pub(super) struct AccessibilityNotice {
    pub(super) id: u64,
    pub(super) level: ToastLevel,
    pub(super) kind: NotificationKind,
    pub(super) text: String,
    pub(super) details: Option<String>,
    pub(super) bounds: Rect,
    pub(super) expanded: bool,
    pub(super) dismissable: bool,
}

#[derive(Default)]
pub(super) struct ToastStack {
    toasts: Vec<Toast>,
    scoped: Vec<Notification>,
    owned_seen: BTreeSet<(NotificationId, u64)>,
    source_revisions: BTreeMap<NotificationId, (String, u64)>,
    resolved_history: Vec<(NotificationId, u64)>,
    next_accessibility_id: u64,
    hits: Vec<(Rect, Hit)>,
    focus: Option<u64>,
    details_open: Option<u64>,
    details_scroll: usize,
    overflow_bounds: Rect,
    overflow_painted: bool,
    details_bounds: Rect,
    details_close_bounds: Rect,
    painted: Vec<u64>,
    hidden: Vec<u64>,
}

impl ToastStack {
    pub(super) fn owned_revision(&mut self, id: &NotificationId, fingerprint: String) -> u64 {
        if let Some((known, revision)) = self.source_revisions.get(id)
            && known == &fingerprint
        {
            return *revision;
        }
        let revision = next_revision();
        self.source_revisions.insert(id.clone(), (fingerprint, revision));
        revision
    }

    fn latest_revision(&self, id: &NotificationId) -> Option<u64> {
        self.toasts
            .iter()
            .filter(|toast| &toast.notification.id == id)
            .map(|toast| toast.notification.revision)
            .chain(
                self.scoped
                    .iter()
                    .filter(|notice| &notice.id == id)
                    .map(|notice| notice.revision),
            )
            .chain(self.owned_seen.iter().filter(|seen| &seen.0 == id).map(|seen| seen.1))
            .chain(
                self.resolved_history
                    .iter()
                    .filter(|seen| &seen.0 == id)
                    .map(|seen| seen.1),
            )
            .max()
    }

    pub(super) fn enqueue(&mut self, notification: Notification, now: Instant) {
        self.enqueue_inner(notification, now, false);
    }

    pub(super) fn enqueue_owned(&mut self, notification: Notification, now: Instant) {
        self.enqueue_inner(notification, now, true);
    }

    fn enqueue_inner(&mut self, mut notification: Notification, now: Instant, owned: bool) {
        if self
            .latest_revision(&notification.id)
            .is_some_and(|revision| revision >= notification.revision)
        {
            return;
        }
        if owned {
            self.owned_seen.retain(|seen| seen.0 != notification.id);
            self.owned_seen.insert((notification.id.clone(), notification.revision));
        }
        if notification.lifetime == NotificationLifetime::Scoped {
            let replaced: Vec<_> = self
                .toasts
                .iter()
                .filter(|toast| toast.notification.id == notification.id)
                .map(|toast| toast.accessibility_id)
                .collect();
            self.toasts.retain(|toast| toast.notification.id != notification.id);
            for root in replaced {
                self.reconcile_removed(root);
            }
            self.scoped.retain(|current| current.id != notification.id);
            self.scoped.push(notification);
            return;
        }
        self.scoped.retain(|current| current.id != notification.id);
        if notification.level != ToastLevel::Info {
            notification.lifetime = NotificationLifetime::Persistent;
        }
        if let Some(toast) = self
            .toasts
            .iter_mut()
            .find(|toast| toast.notification.id == notification.id)
        {
            toast.notification = notification;
            toast.born = now;
            return;
        }
        self.next_accessibility_id = self.next_accessibility_id.wrapping_add(3).max(3);
        self.toasts.push(Toast {
            notification,
            accessibility_id: 90_100_000 + self.next_accessibility_id,
            born: now,
            bounds: Rect::default(),
        });
        while self
            .toasts
            .iter()
            .filter(|toast| toast.notification.lifetime == NotificationLifetime::Transient)
            .count()
            > MAX_TRANSIENT
        {
            if let Some(index) = self
                .toasts
                .iter()
                .position(|toast| toast.notification.lifetime == NotificationLifetime::Transient)
            {
                self.retire(index);
            }
        }
    }

    pub(super) fn push_typed(
        &mut self,
        id: impl Into<NotificationId>,
        revision: u64,
        level: ToastLevel,
        kind: NotificationKind,
        text: impl Into<String>,
        details: Option<String>,
        document: Option<DocumentKey>,
        lifetime: NotificationLifetime,
        now: Instant,
    ) {
        self.enqueue(
            Notification::new(id, revision, level, kind, text, details, document, lifetime),
            now,
        );
    }

    fn retire(&mut self, index: usize) {
        let toast = self.toasts.remove(index);
        self.reconcile_removed(toast.accessibility_id);
        if !self
            .owned_seen
            .contains(&(toast.notification.id.clone(), toast.notification.revision))
        {
            self.resolved_history
                .push((toast.notification.id, toast.notification.revision));
            while self.resolved_history.len() > MAX_RESOLVED_HISTORY {
                self.resolved_history.remove(0);
            }
        }
    }

    fn reconcile_removed(&mut self, root: u64) {
        if self.details_open == Some(root) {
            self.details_open = None;
            self.details_scroll = 0;
        }
        if self.focus.is_some_and(|id| id >= root && id <= root + 2) {
            self.focus = None;
        }
        self.painted.retain(|id| *id != root);
        self.hidden.retain(|id| *id != root);
    }

    pub(super) fn tick(&mut self, now: Instant) -> bool {
        let before = self.toasts.len();
        let expired: Vec<_> = self
            .toasts
            .iter()
            .enumerate()
            .filter_map(|(index, toast)| {
                (toast.notification.lifetime == NotificationLifetime::Transient
                    && now.saturating_duration_since(toast.born) >= INFO_LIFETIME)
                    .then_some(index)
            })
            .collect();
        for index in expired.into_iter().rev() {
            self.retire(index);
        }
        self.toasts.len() != before
    }

    pub(super) fn next_deadline(&self) -> Option<Instant> {
        self.toasts
            .iter()
            .filter(|toast| toast.notification.lifetime == NotificationLifetime::Transient)
            .map(|toast| toast.born + INFO_LIFETIME)
            .min()
    }

    fn dismiss(&mut self, accessibility_id: u64) {
        if accessibility_id == OVERFLOW_ID {
            self.details_open = None;
            return;
        }
        if let Some(index) = self
            .toasts
            .iter()
            .position(|toast| toast.accessibility_id == accessibility_id)
        {
            self.retire(index);
        }
        if self.details_open == Some(accessibility_id) {
            self.details_open = None;
        }
        if self
            .focus
            .is_some_and(|focus| focus >= accessibility_id && focus <= accessibility_id + 2)
        {
            self.focus = None;
        }
    }

    pub(super) fn resolve(&mut self, id: &NotificationId) {
        let removed: Vec<_> = self
            .toasts
            .iter()
            .filter(|toast| &toast.notification.id == id)
            .map(|toast| toast.accessibility_id)
            .collect();
        self.toasts.retain(|toast| &toast.notification.id != id);
        for root in removed {
            self.reconcile_removed(root);
        }
        self.scoped.retain(|notice| &notice.id != id);
        self.owned_seen.retain(|seen| &seen.0 != id);
        self.resolved_history.retain(|seen| &seen.0 != id);
    }

    pub(super) fn retain_owned(&mut self, prefix: &str, active: &[NotificationId]) {
        let obsolete: Vec<_> = self
            .owned_seen
            .iter()
            .filter_map(|seen| (seen.0.0.starts_with(prefix) && !active.contains(&seen.0)).then_some(seen.0.clone()))
            .collect();
        for id in obsolete {
            self.source_revisions.remove(&id);
            self.resolve(&id);
        }
    }

    pub(super) fn clear_document(&mut self, document: DocumentKey) {
        let retired_scoped: Vec<_> = self
            .scoped
            .iter()
            .filter(|notice| notice.document == Some(document))
            .map(|notice| (notice.id.clone(), notice.revision))
            .collect();
        self.scoped.retain(|notice| notice.document != Some(document));
        for retired in retired_scoped {
            self.owned_seen.remove(&retired);
            self.resolved_history.push(retired);
        }
        while self.resolved_history.len() > MAX_RESOLVED_HISTORY {
            self.resolved_history.remove(0);
        }
        let stale: Vec<_> = self
            .toasts
            .iter()
            .enumerate()
            .filter_map(|(index, toast)| {
                (toast.notification.document == Some(document)
                    && toast.notification.lifetime != NotificationLifetime::Persistent)
                    .then_some(index)
            })
            .collect();
        for index in stale.into_iter().rev() {
            self.retire(index);
        }
    }

    pub(super) fn clear_transient(&mut self) {
        for notice in self.scoped.drain(..) {
            let retired = (notice.id, notice.revision);
            self.owned_seen.remove(&retired);
            self.resolved_history.push(retired);
        }
        while self.resolved_history.len() > MAX_RESOLVED_HISTORY {
            self.resolved_history.remove(0);
        }
        let stale: Vec<_> = self
            .toasts
            .iter()
            .enumerate()
            .filter_map(|(index, toast)| {
                (toast.notification.lifetime != NotificationLifetime::Persistent).then_some(index)
            })
            .collect();
        for index in stale.into_iter().rev() {
            self.retire(index);
        }
    }

    pub(super) fn scoped_for(&self, document: DocumentKey) -> Option<&Notification> {
        self.scoped
            .iter()
            .rev()
            .find(|notice| notice.document == Some(document))
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.toasts.is_empty() && self.scoped.is_empty()
    }

    #[cfg(test)]
    pub(super) fn persistent_len(&self) -> usize {
        self.toasts.len()
    }

    pub(super) fn accessibility(&self) -> Vec<AccessibilityNotice> {
        let visible: BTreeSet<_> = self.painted.iter().copied().collect();
        let mut notices: Vec<_> = self
            .toasts
            .iter()
            .filter(|toast| visible.contains(&toast.accessibility_id))
            .map(|toast| AccessibilityNotice {
                id: toast.accessibility_id,
                level: toast.notification.level,
                kind: toast.notification.kind,
                text: toast.notification.text.clone(),
                details: Some(self.full_details(toast.accessibility_id)),
                bounds: toast.bounds,
                expanded: self.details_open == Some(toast.accessibility_id),
                dismissable: true,
            })
            .collect();
        if self.overflow_painted {
            notices.push(AccessibilityNotice {
                id: OVERFLOW_ID,
                level: ToastLevel::Warning,
                kind: NotificationKind::Outcome,
                text: format!("{} more retained notifications", self.hidden.len()),
                details: Some(self.overflow_details()),
                bounds: self.overflow_bounds,
                expanded: self.details_open == Some(OVERFLOW_ID),
                dismissable: false,
            });
        }
        notices
    }

    pub(super) fn accessibility_focus(&self) -> Option<u64> {
        self.focus
    }

    pub(super) fn blur(&mut self) {
        self.focus = None;
    }

    pub(super) fn details_open(&self) -> bool {
        self.details_open.is_some()
    }

    pub(super) fn open_details(&mut self, id: u64) -> bool {
        if id != OVERFLOW_ID && !self.toasts.iter().any(|toast| toast.accessibility_id == id) {
            return false;
        }
        if id == OVERFLOW_ID && self.hidden.is_empty() {
            return false;
        }
        self.details_open = Some(id);
        self.details_scroll = 0;
        self.focus = Some(id);
        true
    }

    pub(super) fn close_details(&mut self) {
        self.details_open = None;
        self.details_scroll = 0;
        self.focus = None;
    }

    pub(super) fn details_semantics(&self) -> Option<(String, Rect, Rect)> {
        let id = self.details_open?;
        Some((self.full_details(id), self.details_bounds, self.details_close_bounds))
    }

    pub(super) fn details_pointer_close(&self, point: Point) -> bool {
        self.details_open.is_some() && self.details_close_bounds.contains(point)
    }

    pub(super) fn details_contains(&self, point: Point) -> bool {
        self.details_open.is_some() && self.details_bounds.contains(point)
    }

    pub(super) fn scroll_details(&mut self, down: bool) {
        if self.details_open.is_some() {
            self.details_scroll = if down {
                self.details_scroll.saturating_add(3)
            } else {
                self.details_scroll.saturating_sub(3)
            };
        }
    }

    pub(super) fn accessibility_action(&mut self, id: u64, invoke: bool) -> Option<ToastAction> {
        let root = if self.overflow_painted && (id == OVERFLOW_ID || id == OVERFLOW_ID + 1) {
            Some(OVERFLOW_ID)
        } else {
            self.toasts.iter().find_map(|toast| {
                (self.painted.contains(&toast.accessibility_id)
                    && id >= toast.accessibility_id
                    && id <= toast.accessibility_id + 2)
                    .then_some(toast.accessibility_id)
            })
        };
        let Some(root) = root else {
            return None;
        };
        self.focus = Some(id);
        if invoke {
            if id == root + 2 {
                return Some(ToastAction::Dismiss(root));
            } else if id == root || id == root + 1 {
                return Some(ToastAction::OpenDetails(root));
            }
        }
        Some(ToastAction::Consumed)
    }

    pub(super) fn key(&mut self, key: &Key, backwards: bool) -> Option<ToastAction> {
        let mut roots = self.painted.clone();
        if self.overflow_painted {
            roots.insert(0, OVERFLOW_ID);
        }
        if roots.is_empty() {
            self.focus = None;
            return None;
        }
        if self.focus.is_none() && *key != Key::Named(NamedKey::F8) {
            return None;
        }
        match key {
            Key::Named(NamedKey::F8) => self.focus = Some(*roots.last().unwrap()),
            Key::Named(NamedKey::Tab) => {
                let current = self
                    .focus
                    .and_then(|id| roots.iter().position(|candidate| *candidate == id));
                let next = if backwards {
                    current.unwrap_or(0).checked_sub(1).unwrap_or(roots.len() - 1)
                } else {
                    current.map_or(0, |index| (index + 1) % roots.len())
                };
                self.focus = Some(roots[next]);
            }
            Key::Named(NamedKey::Enter) => {
                let focus = self.focus.unwrap_or(*roots.last().unwrap());
                return Some(ToastAction::OpenDetails(self.root_for_action(focus).unwrap_or(focus)));
            }
            Key::Character(value) if value == " " => {
                let focus = self.focus.unwrap_or(*roots.last().unwrap());
                return Some(ToastAction::OpenDetails(self.root_for_action(focus).unwrap_or(focus)));
            }
            Key::Named(NamedKey::Delete) => {
                let focus = self.focus.unwrap_or(*roots.last().unwrap());
                return Some(ToastAction::Dismiss(self.root_for_action(focus).unwrap_or(focus)));
            }
            Key::Named(NamedKey::Escape) => self.focus = None,
            Key::Named(NamedKey::PageDown | NamedKey::ArrowDown) if self.details_open.is_some() => {
                self.details_scroll = self.details_scroll.saturating_add(1);
            }
            Key::Named(NamedKey::PageUp | NamedKey::ArrowUp) if self.details_open.is_some() => {
                self.details_scroll = self.details_scroll.saturating_sub(1);
            }
            _ => return None,
        }
        Some(ToastAction::Consumed)
    }

    fn root_for_action(&self, id: u64) -> Option<u64> {
        if id >= OVERFLOW_ID && id <= OVERFLOW_ID + 2 {
            Some(OVERFLOW_ID)
        } else {
            self.toasts.iter().find_map(|toast| {
                (id >= toast.accessibility_id && id <= toast.accessibility_id + 2).then_some(toast.accessibility_id)
            })
        }
    }

    pub(super) fn hit(&mut self, point: Point) -> Option<ToastAction> {
        let Some((_, hit)) = self.hits.iter().find(|(rect, _)| rect.contains(point)).copied() else {
            return None;
        };
        Some(match hit {
            Hit::Dismiss(id) => ToastAction::Dismiss(id),
            Hit::Details(id) => ToastAction::OpenDetails(id),
        })
    }

    pub(super) fn perform(&mut self, action: ToastAction) {
        if let ToastAction::Dismiss(id) = action {
            self.dismiss(id);
        }
    }

    fn overflow_details(&self) -> String {
        self.toasts
            .iter()
            .filter(|toast| self.hidden.contains(&toast.accessibility_id))
            .map(|toast| {
                toast.notification.details.as_ref().map_or_else(
                    || toast.notification.text.clone(),
                    |details| format!("{}\n{}", toast.notification.text, details),
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn full_details(&self, id: u64) -> String {
        if id == OVERFLOW_ID {
            return self.overflow_details();
        }
        self.toasts
            .iter()
            .find(|toast| toast.accessibility_id == id)
            .map(|toast| match &toast.notification.details {
                Some(details) => format!("{}\n\n{details}", toast.notification.text),
                None => toast.notification.text.clone(),
            })
            .unwrap_or_default()
    }

    pub(super) fn draw(
        &mut self,
        renderer: &mut impl TextBackend,
        width: f32,
        height: f32,
        theme: UiTheme,
        ops: &mut Vec<DrawOp>,
    ) {
        self.hits.clear();
        self.painted.clear();
        self.hidden.clear();
        self.overflow_bounds = Rect::default();
        self.overflow_painted = false;
        let toast_width = (width - 32.0).min(WIDTH).max(80.0).min(width.max(0.0));
        let x = ((width - toast_width) / 2.0).max(0.0);
        let mut y = height - bareline_ui::STATUS_HEIGHT - GAP;
        let available = ((y - bareline_ui::TAB_HEIGHT) / (ROW_HEIGHT + GAP)).floor().max(0.0) as usize;
        let capacity = available.min(MAX_VISIBLE);
        let visible_count =
            if self.toasts.len() > capacity || (self.details_open == Some(OVERFLOW_ID) && !self.toasts.is_empty()) {
                capacity.saturating_sub(1)
            } else {
                capacity
            };
        let ids: Vec<_> = self
            .toasts
            .iter()
            .rev()
            .take(visible_count)
            .map(|toast| toast.accessibility_id)
            .collect();
        self.painted.extend(ids.iter().copied());
        self.hidden = self
            .toasts
            .iter()
            .map(|toast| toast.accessibility_id)
            .filter(|id| !self.painted.contains(id))
            .collect();
        if let Some(focus) = self.focus {
            let root = self.root_for_action(focus).unwrap_or(focus);
            if !self.painted.contains(&root) {
                self.focus = (capacity > 0 && !self.hidden.is_empty()).then_some(OVERFLOW_ID);
            }
        }
        for id in ids {
            y -= ROW_HEIGHT;
            if y < bareline_ui::TAB_HEIGHT {
                break;
            }
            let Some(toast) = self.toasts.iter_mut().find(|toast| toast.accessibility_id == id) else {
                continue;
            };
            toast.bounds = bareline_ui::rect(x, y, toast_width, ROW_HEIGHT);
            draw_row(
                renderer,
                toast.bounds,
                &toast.notification.text,
                toast.notification.level,
                true,
                self.focus.is_some_and(|focus| focus >= id && focus <= id + 2),
                theme,
                ops,
            );
            self.hits.push((
                bareline_ui::rect(x + 14.0, y + ROW_HEIGHT - 23.0, 76.0, 19.0),
                Hit::Details(id),
            ));
            self.hits.push((
                bareline_ui::rect(x + toast_width - 28.0, y + 5.0, 20.0, 20.0),
                Hit::Dismiss(id),
            ));
            y -= GAP;
        }
        if !self.hidden.is_empty() && capacity > 0 && y >= bareline_ui::TAB_HEIGHT + ROW_HEIGHT {
            y -= ROW_HEIGHT;
            let bounds = bareline_ui::rect(x, y, toast_width, ROW_HEIGHT);
            self.overflow_bounds = bounds;
            self.overflow_painted = true;
            let hidden = self.hidden.len();
            draw_row(
                renderer,
                bounds,
                &format!("{hidden} more retained notifications"),
                ToastLevel::Warning,
                true,
                self.focus
                    .is_some_and(|focus| focus >= OVERFLOW_ID && focus <= OVERFLOW_ID + 2),
                theme,
                ops,
            );
            self.hits.push((bounds, Hit::Details(OVERFLOW_ID)));
        }
        if let Some(id) = self.details_open {
            self.draw_details(renderer, id, width, height, theme, ops);
        }
    }

    fn draw_details(
        &mut self,
        renderer: &mut impl TextBackend,
        id: u64,
        width: f32,
        height: f32,
        theme: UiTheme,
        ops: &mut Vec<DrawOp>,
    ) {
        let content = self.full_details(id);
        let panel_width = (width - 32.0).min(720.0).max(80.0).min(width.max(0.0));
        let panel_height = (height - 96.0).min(420.0).max(100.0).min(height.max(0.0));
        let bounds = bareline_ui::rect(
            (width - panel_width).max(0.0) / 2.0,
            (height - panel_height).max(0.0) / 2.0,
            panel_width,
            panel_height,
        );
        self.details_bounds = bounds;
        self.details_close_bounds = bareline_ui::rect(bounds.x + panel_width - 84.0, bounds.y + 10.0, 68.0, 26.0);
        let palette = theme.toast(ToastLevel::Info);
        ops.push(DrawOp::FillRounded(bounds, palette.surface, 8.0));
        ops.push(DrawOp::StrokeRounded(bounds, palette.border, 8.0, 1.0));
        bareline_ui::text(
            ops,
            bounds.x + 16.0,
            bounds.y + 12.0,
            "Notification details",
            15.0,
            palette.text,
        );
        ops.push(DrawOp::StrokeRounded(
            self.details_close_bounds,
            palette.border,
            5.0,
            1.0,
        ));
        bareline_ui::text(
            ops,
            self.details_close_bounds.x + 14.0,
            self.details_close_bounds.y + 5.0,
            "Close",
            12.0,
            palette.text,
        );
        let lines = wrap_measured(renderer, &content, 13.0, (panel_width - 32.0).max(20.0));
        let visible = ((panel_height - 58.0) / 18.0).max(1.0) as usize;
        self.details_scroll = self.details_scroll.min(lines.len().saturating_sub(visible));
        for (row, line) in lines.iter().skip(self.details_scroll).take(visible).enumerate() {
            bareline_ui::text(
                ops,
                bounds.x + 16.0,
                bounds.y + 39.0 + row as f32 * 18.0,
                line,
                13.0,
                palette.text,
            );
        }
        bareline_ui::text(
            ops,
            bounds.x + 16.0,
            bounds.y + panel_height - 20.0,
            "Page Up/Down scrolls · Escape closes",
            11.0,
            palette.muted,
        );
    }
}

fn draw_row(
    renderer: &mut impl TextBackend,
    bounds: Rect,
    value: &str,
    level: ToastLevel,
    details: bool,
    focused: bool,
    theme: UiTheme,
    ops: &mut Vec<DrawOp>,
) {
    let palette = theme.toast(level);
    ops.push(DrawOp::FillRounded(bounds, palette.surface, 8.0));
    ops.push(DrawOp::StrokeRounded(
        bounds,
        if focused { palette.accent } else { palette.border },
        8.0,
        if focused { 2.0 } else { 1.0 },
    ));
    ops.push(DrawOp::FillRounded(
        bareline_ui::rect(bounds.x, bounds.y + 6.0, 4.0, bounds.height - 12.0),
        palette.accent,
        2.0,
    ));
    for (row, line) in wrap_measured(renderer, value, 13.0, (bounds.width - 50.0).max(20.0))
        .into_iter()
        .take(2)
        .enumerate()
    {
        bareline_ui::text(
            ops,
            bounds.x + 16.0,
            bounds.y + 7.0 + row as f32 * 16.0,
            &line,
            13.0,
            palette.text,
        );
    }
    if details {
        bareline_ui::text(
            ops,
            bounds.x + 16.0,
            bounds.y + bounds.height - 20.0,
            "Details…",
            12.0,
            palette.accent,
        );
    }
    bareline_ui::text(
        ops,
        bounds.x + bounds.width - 23.0,
        bounds.y + 8.0,
        "×",
        15.0,
        palette.muted,
    );
}

fn wrap_measured(renderer: &mut impl TextBackend, text: &str, size: f32, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for source in text.lines() {
        let mut rest = source;
        while !rest.is_empty() {
            let boundaries: Vec<_> = rest
                .char_indices()
                .map(|(index, _)| index)
                .chain(std::iter::once(rest.len()))
                .collect();
            let mut low = 1usize;
            let mut high = boundaries.len();
            while low < high {
                let mid = (low + high) / 2;
                let fits = renderer
                    .measure_text(&rest[..boundaries[mid]], size)
                    .map_or(true, |measured| measured.0 <= width);
                if fits {
                    low = mid + 1;
                } else {
                    high = mid;
                }
            }
            let fit = low.saturating_sub(1).max(1).min(boundaries.len() - 1);
            let mut split = boundaries[fit];
            if split < rest.len()
                && let Some(space) = rest[..split]
                    .char_indices()
                    .rev()
                    .find_map(|(index, ch)| (index > 0 && ch.is_whitespace()).then_some(index))
            {
                split = space;
            }
            lines.push(rest[..split].trim_end().to_owned());
            rest = rest[split..].trim_start();
        }
        if source.is_empty() {
            lines.push(String::new());
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    fn notice(id: impl Into<NotificationId>, revision: u64, lifetime: NotificationLifetime) -> Notification {
        Notification::new(
            id,
            revision,
            ToastLevel::Error,
            NotificationKind::Outcome,
            "message",
            None,
            None,
            lifetime,
        )
    }

    #[test]
    fn persistent_ownership_is_separate_from_visible_capacity() {
        let now = Instant::now();
        let mut stack = ToastStack::default();
        for revision in 0..40 {
            stack.enqueue_owned(
                notice(format!("owned-{revision}"), revision, NotificationLifetime::Persistent),
                now,
            );
        }
        assert_eq!(stack.toasts.len(), 40);
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        stack.draw(&mut renderer, 900.0, 800.0, UiTheme::default(), &mut Vec::new());
        assert_eq!(stack.painted.len(), MAX_VISIBLE - 1);
        assert!(stack.accessibility().iter().any(|notice| notice.id == OVERFLOW_ID));
        for revision in 0..40 {
            stack.enqueue_owned(
                notice(format!("owned-{revision}"), revision, NotificationLifetime::Persistent),
                now,
            );
        }
        assert_eq!(
            stack.toasts.len(),
            40,
            "source synchronization must not replay retained outcomes"
        );
        stack.enqueue_owned(notice("owned-10", 9, NotificationLifetime::Persistent), now);
        assert_eq!(
            stack
                .toasts
                .iter()
                .find(|toast| toast.notification.id.0 == "owned-10")
                .unwrap()
                .notification
                .revision,
            10,
            "an older revision must not replace current state",
        );
        stack.dismiss(stack.toasts[0].accessibility_id);
        stack.enqueue_owned(notice("owned-0", 0, NotificationLifetime::Persistent), now);
        assert_eq!(stack.toasts.len(), 39, "acknowledged owned outcomes stay acknowledged");
        let active: Vec<_> = (1..40)
            .map(|revision| NotificationId(format!("owned-{revision}")))
            .collect();
        stack.retain_owned("owned-", &active);
        stack.enqueue_owned(notice("owned-0", 0, NotificationLifetime::Persistent), now);
        assert_eq!(stack.toasts.len(), 40, "resolution permits a later identical event");
    }

    #[test]
    fn scoped_progress_is_typed_and_document_owned() {
        let now = Instant::now();
        let mut stack = ToastStack::default();
        let mut first = Notification::new(
            "recovery-1",
            1,
            ToastLevel::Info,
            NotificationKind::Progress,
            "Preparing",
            None,
            Some((1, 1)),
            NotificationLifetime::Scoped,
        );
        stack.enqueue(first.clone(), now);
        first.revision = 2;
        first.text = "Ready".into();
        stack.enqueue(first, now);
        stack.enqueue(
            Notification::new(
                "recovery-2",
                1,
                ToastLevel::Info,
                NotificationKind::Progress,
                "Preparing",
                None,
                Some((2, 1)),
                NotificationLifetime::Scoped,
            ),
            now,
        );
        assert_eq!(stack.scoped.len(), 2);
        assert_eq!(
            stack
                .scoped
                .iter()
                .find(|notice| notice.id.0 == "recovery-1")
                .unwrap()
                .revision,
            2
        );
        stack.clear_document((1, 1));
        assert_eq!(stack.scoped.len(), 1);
        assert_eq!(stack.scoped[0].document, Some((2, 1)));
    }

    #[test]
    fn document_generation_close_expires_progress_but_retains_failure() {
        let now = Instant::now();
        let mut stack = ToastStack::default();
        stack.enqueue(
            Notification::new(
                "recovery-progress",
                1,
                ToastLevel::Info,
                NotificationKind::Progress,
                "Preparing",
                None,
                Some((7, 11)),
                NotificationLifetime::Scoped,
            ),
            now,
        );
        stack.enqueue(
            Notification::new(
                "recovery-failed",
                1,
                ToastLevel::Error,
                NotificationKind::Outcome,
                "Recovery unavailable",
                Some("complete failure".into()),
                Some((7, 11)),
                NotificationLifetime::Persistent,
            ),
            now,
        );
        stack.clear_document((7, 10));
        assert_eq!(
            stack.scoped.len(),
            1,
            "an older generation cannot retire current progress"
        );
        stack.clear_document((7, 11));
        assert!(stack.scoped.is_empty());
        assert_eq!(
            stack.toasts.len(),
            1,
            "closed-document failure keeps operation ownership"
        );
    }

    #[test]
    fn warning_wording_does_not_choose_level_and_details_are_complete() {
        let now = Instant::now();
        let mut stack = ToastStack::default();
        let details = "C:\\長い名前\\stage-copy-with-an-unbroken-name-that-must-remain-complete.tmp";
        let mut event = Notification::new(
            "save",
            1,
            ToastLevel::Warning,
            NotificationKind::Outcome,
            "Everything succeeded",
            Some(details.into()),
            None,
            NotificationLifetime::Transient,
        );
        stack.enqueue(event.clone(), now);
        assert_eq!(stack.toasts[0].notification.level, ToastLevel::Warning);
        assert_eq!(stack.toasts[0].notification.lifetime, NotificationLifetime::Persistent);
        stack.draw(
            &mut bareline_renderer_recording::RecordingBackend::default(),
            800.0,
            600.0,
            UiTheme::default(),
            &mut Vec::new(),
        );
        assert!(stack.accessibility()[0].details.as_deref().unwrap().ends_with(details));
        event.revision = 2;
        event.level = ToastLevel::Error;
        stack.enqueue(event, now);
        assert_eq!(stack.toasts.len(), 1);
        assert_eq!(stack.toasts[0].notification.revision, 2);
    }

    #[test]
    fn narrow_geometry_and_keyboard_details_keep_complete_unicode_value() {
        let now = Instant::now();
        let details = "C:\\長い名前\\an-unbroken-file-name-that-does-not-fit-in-one-row.tmp";
        let mut event = notice("details", 1, NotificationLifetime::Persistent);
        event.details = Some(details.into());
        let mut stack = ToastStack::default();
        stack.enqueue(event, now);
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        let mut ops = Vec::new();
        stack.draw(&mut renderer, 120.0, 240.0, UiTheme::default(), &mut ops);
        assert!(stack.toasts[0].bounds.x >= 0.0);
        assert!(stack.toasts[0].bounds.x + stack.toasts[0].bounds.width <= 120.0);
        assert!(stack.key(&Key::Named(NamedKey::F8), false).is_some());
        let action = stack.key(&Key::Named(NamedKey::Enter), false).unwrap();
        assert!(matches!(action, ToastAction::OpenDetails(_)));
        assert!(stack.accessibility()[0].details.as_deref().unwrap().ends_with(details));
        let details_id = stack.accessibility()[0].id + 1;
        assert!(matches!(
            stack.accessibility_action(details_id, true),
            Some(ToastAction::OpenDetails(_))
        ));
    }

    #[test]
    fn measured_wrapping_is_unicode_boundary_safe_and_keeps_full_summary() {
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        let text = "\u{3000}長い通知 without explicit details";
        let lines = wrap_measured(&mut renderer, text, 13.0, 12.0);
        // Layout discards whitespace used only at a visual line boundary; the
        // complete source remains available through the Details value below.
        assert_eq!(lines.concat().replace(' ', ""), text.trim_start().replace(' ', ""));
        let mut stack = ToastStack::default();
        stack.enqueue(notice("unicode", 1, NotificationLifetime::Persistent), Instant::now());
        stack.toasts[0].notification.text = text.into();
        stack.draw(&mut renderer, 160.0, 180.0, UiTheme::default(), &mut Vec::new());
        assert_eq!(stack.accessibility()[0].details.as_deref(), Some(text));
    }

    #[test]
    fn height_clipped_rows_are_only_reachable_through_overflow() {
        let mut stack = ToastStack::default();
        for revision in 0..8 {
            stack.enqueue(
                notice(format!("n-{revision}"), revision, NotificationLifetime::Persistent),
                Instant::now(),
            );
        }
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        stack.draw(&mut renderer, 320.0, 180.0, UiTheme::default(), &mut Vec::new());
        let accessible = stack.accessibility();
        assert_eq!(accessible.len(), stack.painted.len() + 1);
        assert!(accessible.iter().any(|notice| notice.id == OVERFLOW_ID));
        assert_eq!(stack.hidden.len() + stack.painted.len(), stack.toasts.len());
    }

    #[test]
    fn zero_row_capacity_does_not_publish_an_unpainted_overflow_control() {
        let mut stack = ToastStack::default();
        stack.enqueue(notice("hidden", 1, NotificationLifetime::Persistent), Instant::now());
        stack.draw(
            &mut bareline_renderer_recording::RecordingBackend::default(),
            240.0,
            50.0,
            UiTheme::default(),
            &mut Vec::new(),
        );
        assert!(stack.accessibility().is_empty());
        assert!(stack.key(&Key::Named(NamedKey::F8), false).is_none());
        assert_eq!(stack.accessibility_focus(), None);
    }

    #[test]
    fn pointer_details_has_close_geometry_and_removed_owner_releases_focus() {
        let mut stack = ToastStack::default();
        stack.enqueue(notice("owned", 1, NotificationLifetime::Persistent), Instant::now());
        let mut renderer = bareline_renderer_recording::RecordingBackend::default();
        stack.draw(&mut renderer, 360.0, 300.0, UiTheme::default(), &mut Vec::new());
        let root = stack.painted[0];
        assert!(stack.open_details(root));
        stack.draw(&mut renderer, 360.0, 300.0, UiTheme::default(), &mut Vec::new());
        assert!(stack.details_contains(Point {
            x: stack.details_bounds.x + 1.0,
            y: stack.details_bounds.y + 1.0,
        }));
        assert!(stack.details_pointer_close(Point {
            x: stack.details_close_bounds.x + 1.0,
            y: stack.details_close_bounds.y + 1.0,
        }));
        stack.resolve(&NotificationId::from("owned"));
        assert!(!stack.details_open());
        assert_eq!(stack.accessibility_focus(), None);
    }
}
