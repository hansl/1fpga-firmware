//! Listener registry + focus stack, GC-tracked so JS callbacks stay
//! alive while subscribed.
//!
//! Inserted into the Boa context the same way `UiState` is, then
//! retrieved via `context.get_data::<InputState>()` from the host
//! functions and the frame-loop dispatcher.

use std::cell::RefCell;
use std::rc::Rc;

use boa_engine::object::builtins::JsFunction;
use boa_macros::{Finalize, JsData, Trace};

use crate::input::events::InputSource;

/// Listener identifier, returned by `add*Listener` so JS can later
/// unsubscribe. Monotonically increasing; never reused within a
/// session.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct ListenerId(pub u32);

/// Where a listener is willing to fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenerScope {
    /// Always fires regardless of which subtree is focused.
    Global,
    /// Only fires when the focused node id matches `node_id` or one of
    /// its ancestors. (For now we collapse this to "exact match" — a
    /// proper ancestor walk arrives with the React `useFocus` hook.)
    Focused {
        node_id: u32,
    },
}

#[derive(Debug, Clone)]
pub enum ListenerKind {
    Intent { name: String },
    Raw { source: InputSource },
}

/// Storage owns a `Rc<RefCell<...>>` instead of `Gc<GcRefCell<...>>`
/// because the listeners we hold (JsFunction handles) can be cheaply
/// `clone()`d at dispatch time — we snapshot the function list and
/// drop the borrow before invoking JS, avoiding both GC drama and
/// iteration-during-mutation. The Boa context still holds the
/// `JsFunction`s alive via the snapshots passed to `Function::call`.
#[derive(Default, Clone, Trace, Finalize, JsData)]
pub struct InputState {
    #[unsafe_ignore_trace]
    inner: Rc<RefCell<InputStateInner>>,
}

#[derive(Default)]
struct InputStateInner {
    listeners: Vec<Listener>,
    focus_stack: Vec<u32>,
    next_id: u32,
}

struct Listener {
    id: ListenerId,
    kind: ListenerKind,
    scope: ListenerScope,
    handler: JsFunction,
}

impl InputState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a listener. Returns its id; pass to [`Self::remove`].
    pub fn add(
        &self,
        kind: ListenerKind,
        scope: ListenerScope,
        handler: JsFunction,
    ) -> ListenerId {
        let mut inner = self.inner.borrow_mut();
        let id = ListenerId(inner.next_id);
        inner.next_id = inner.next_id.wrapping_add(1);
        let kind_dbg = format!("{:?}", kind);
        inner.listeners.push(Listener {
            id,
            kind,
            scope,
            handler,
        });
        let total = inner.listeners.len();
        tracing::info!("input: + listener {} {} (total={})", id.0, kind_dbg, total);
        id
    }

    pub fn remove(&self, id: ListenerId) -> bool {
        let mut inner = self.inner.borrow_mut();
        let before = inner.listeners.len();
        inner.listeners.retain(|l| l.id != id);
        inner.listeners.len() != before
    }

    /// Snapshot every intent listener whose name matches and whose
    /// scope passes, returning their `JsFunction`s. The caller calls
    /// each with the appropriate args after dropping the borrow.
    pub fn snapshot_intent(&self, name: &str) -> Vec<JsFunction> {
        let inner = self.inner.borrow();
        let focus = inner.focus_stack.last().copied();
        inner
            .listeners
            .iter()
            .filter(|l| matches!(&l.kind, ListenerKind::Intent { name: n } if n == name))
            .filter(|l| scope_matches(l.scope, focus))
            .map(|l| l.handler.clone())
            .collect()
    }

    /// Same as [`snapshot_intent`] but for raw input listeners,
    /// filtered by source.
    pub fn snapshot_raw(&self, source: InputSource) -> Vec<JsFunction> {
        let inner = self.inner.borrow();
        let focus = inner.focus_stack.last().copied();
        inner
            .listeners
            .iter()
            .filter(|l| matches!(&l.kind, ListenerKind::Raw { source: s } if *s == source))
            .filter(|l| scope_matches(l.scope, focus))
            .map(|l| l.handler.clone())
            .collect()
    }

    // --- Focus stack -------------------------------------------------

    pub fn push_focus(&self, node_id: u32) {
        self.inner.borrow_mut().focus_stack.push(node_id);
    }

    pub fn pop_focus(&self) -> Option<u32> {
        self.inner.borrow_mut().focus_stack.pop()
    }

    pub fn set_focus(&self, node_id: u32) {
        let mut inner = self.inner.borrow_mut();
        inner.focus_stack.clear();
        inner.focus_stack.push(node_id);
    }

    pub fn focus(&self) -> Option<u32> {
        self.inner.borrow().focus_stack.last().copied()
    }

    /// Diagnostics: number of registered listeners.
    pub fn listener_count(&self) -> usize {
        self.inner.borrow().listeners.len()
    }
}

fn scope_matches(scope: ListenerScope, focus: Option<u32>) -> bool {
    match scope {
        ListenerScope::Global => true,
        ListenerScope::Focused { node_id } => focus.is_some_and(|f| f == node_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_stack_lifo() {
        let s = InputState::new();
        s.push_focus(1);
        s.push_focus(2);
        s.push_focus(3);
        assert_eq!(s.focus(), Some(3));
        assert_eq!(s.pop_focus(), Some(3));
        assert_eq!(s.focus(), Some(2));
    }

    #[test]
    fn set_focus_replaces_stack() {
        let s = InputState::new();
        s.push_focus(1);
        s.push_focus(2);
        s.set_focus(99);
        assert_eq!(s.focus(), Some(99));
    }
}
