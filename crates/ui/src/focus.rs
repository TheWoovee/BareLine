// SPDX-License-Identifier: MPL-2.0
//! Focus is scoped to the active layer. Popup dismissal restores its invoker.
use crate::ViewId;
use crate::controls::UiEvent;
use bareline_renderer::Rect;

#[derive(Clone, Copy)]
pub struct RouteTarget {
    pub id: ViewId,
    pub parent: Option<ViewId>,
    pub bounds: Rect,
    pub enabled: bool,
}
#[derive(Default)]
pub struct DispatchResult {
    pub handled: bool,
    pub invalidated: Option<Rect>,
}
#[derive(Default)]
pub struct RoutedEvent {
    pub handled: bool,
    pub invalidated: Vec<Rect>,
}
/// Routing invokes the top popup first, then focus, hit target, and ancestors.
/// Owners translate events/actions to commands; the router performs no painting.
#[derive(Default)]
pub struct EventRouter {
    pub popup_capture: Option<ViewId>,
    pub pointer_capture: Option<ViewId>,
}
impl EventRouter {
    pub fn dispatch(
        &mut self,
        event: UiEvent,
        focused: Option<ViewId>,
        targets: &[RouteTarget],
        handler: impl FnMut(ViewId, UiEvent) -> DispatchResult,
    ) -> Vec<Rect> {
        self.route(event, focused, targets, handler).invalidated
    }
    /// Native owners need the handled result to avoid dispatching the same key
    /// again as an editor command. The original invalidation-only API remains.
    pub fn route(
        &mut self,
        event: UiEvent,
        focused: Option<ViewId>,
        targets: &[RouteTarget],
        mut handler: impl FnMut(ViewId, UiEvent) -> DispatchResult,
    ) -> RoutedEvent {
        let pointer = match event {
            UiEvent::PointerMove(p) | UiEvent::PointerDown(p) | UiEvent::PointerUp(p) => Some(p),
            _ => None,
        };
        let hit = pointer.and_then(|p| {
            self.pointer_capture.or_else(|| {
                targets
                    .iter()
                    .rev()
                    .find(|t| t.enabled && t.bounds.contains(p))
                    .map(|t| t.id)
            })
        });
        let mut order = Vec::new();
        for id in [self.popup_capture, focused, hit].into_iter().flatten() {
            if !order.contains(&id) {
                order.push(id);
            }
        }
        let mut parent = hit
            .or(focused)
            .and_then(|id| targets.iter().find(|t| t.id == id))
            .and_then(|t| t.parent);
        let mut ancestors = Vec::new();
        while let Some(id) = parent {
            if ancestors.contains(&id) || ancestors.len() >= targets.len() {
                break;
            }
            ancestors.push(id);
            if !order.contains(&id) {
                order.push(id);
            }
            parent = targets.iter().find(|t| t.id == id).and_then(|t| t.parent);
        }
        let mut invalidated = Vec::new();
        let mut handled = false;
        for id in order {
            if !targets.iter().any(|t| t.id == id && t.enabled) {
                continue;
            }
            let result = handler(id, event);
            if let Some(region) = result.invalidated {
                invalidated.push(region);
            }
            if result.handled {
                handled = true;
                break;
            }
        }
        if matches!(event, UiEvent::PointerUp(_) | UiEvent::Focus(false)) {
            self.pointer_capture = None;
        }
        RoutedEvent {
            handled,
            invalidated,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FocusTarget {
    pub id: ViewId,
    pub enabled: bool,
}

#[derive(Default)]
struct Layer {
    targets: Vec<FocusTarget>,
    focused: Option<ViewId>,
    invoker: Option<ViewId>,
}

#[derive(Default)]
pub struct FocusChain {
    base: Layer,
    popups: Vec<Layer>,
}
impl FocusChain {
    fn layer(&self) -> &Layer {
        self.popups.last().unwrap_or(&self.base)
    }
    fn layer_mut(&mut self) -> &mut Layer {
        self.popups.last_mut().unwrap_or(&mut self.base)
    }
    pub fn focused(&self) -> Option<ViewId> {
        self.layer().focused
    }
    /// Replacing targets also clears focus on a control that became disabled/hidden.
    pub fn set_targets(&mut self, targets: Vec<FocusTarget>) {
        let layer = self.layer_mut();
        layer.targets = targets;
        if !layer
            .targets
            .iter()
            .any(|t| t.enabled && Some(t.id) == layer.focused)
        {
            layer.focused = None;
        }
    }
    pub fn focus(&mut self, id: ViewId) -> bool {
        let layer = self.layer_mut();
        if !layer.targets.iter().any(|t| t.id == id && t.enabled) {
            return false;
        }
        layer.focused = Some(id);
        true
    }
    pub fn traverse(&mut self, backwards: bool) -> Option<ViewId> {
        let layer = self.layer_mut();
        let len = layer.targets.len();
        let current = layer
            .targets
            .iter()
            .position(|t| Some(t.id) == layer.focused);
        for step in 1..=len {
            let index = match current {
                Some(i) if backwards => (i + len - step) % len,
                Some(i) => (i + step) % len,
                None if backwards => len - step,
                None => step - 1,
            };
            if layer.targets[index].enabled {
                layer.focused = Some(layer.targets[index].id);
                return layer.focused;
            }
        }
        layer.focused = None;
        None
    }
    pub fn open_layer(&mut self, invoker: ViewId, targets: Vec<FocusTarget>) {
        self.popups.push(Layer {
            targets,
            invoker: Some(invoker),
            focused: None,
        });
        self.traverse(false);
    }
    /// False means no popup was open; callers may then handle Escape in the owner.
    pub fn close_layer(&mut self) -> bool {
        let Some(layer) = self.popups.pop() else {
            return false;
        };
        if let Some(invoker) = layer.invoker
            && !self.focus(invoker)
        {
            self.traverse(false);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn target(id: u64, enabled: bool) -> FocusTarget {
        FocusTarget {
            id: ViewId(id),
            enabled,
        }
    }
    #[test]
    fn traversal_skips_disabled_and_nested_layers_restore_their_invokers() {
        let mut focus = FocusChain::default();
        focus.set_targets(vec![target(1, true), target(2, false), target(3, true)]);
        for (backwards, expected) in [(false, 1), (false, 3), (false, 1), (true, 3)] {
            assert_eq!(focus.traverse(backwards), Some(ViewId(expected)));
        }
        focus.open_layer(ViewId(3), vec![target(4, false), target(5, true)]);
        assert_eq!(focus.traverse(true), Some(ViewId(5)));
        focus.open_layer(ViewId(5), vec![target(6, true)]);
        assert!(focus.close_layer());
        assert_eq!(focus.focused(), Some(ViewId(5)));
        assert!(focus.close_layer());
        assert_eq!(focus.focused(), Some(ViewId(3)));
        focus.set_targets(vec![target(3, false)]);
        assert_eq!(focus.traverse(false), None);
        assert!(!focus.close_layer());
    }
}
