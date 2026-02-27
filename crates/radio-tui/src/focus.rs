//! FocusRing — manages keyboard focus cycling between components.

use crate::action::ComponentId;

pub struct FocusRing {
    items: Vec<ComponentId>,
    current: usize,
}

impl FocusRing {
    pub fn new(items: Vec<ComponentId>) -> Self {
        Self { items, current: 0 }
    }

    pub fn current(&self) -> Option<ComponentId> {
        self.items.get(self.current).copied()
    }

    pub fn next(&mut self) -> Option<ComponentId> {
        if self.items.is_empty() {
            return None;
        }
        self.current = (self.current + 1) % self.items.len();
        self.current()
    }

    pub fn prev(&mut self) -> Option<ComponentId> {
        if self.items.is_empty() {
            return None;
        }
        self.current = if self.current == 0 {
            self.items.len() - 1
        } else {
            self.current - 1
        };
        self.current()
    }

    pub fn set(&mut self, id: ComponentId) {
        if let Some(pos) = self.items.iter().position(|&x| x == id) {
            self.current = pos;
        }
    }

    pub fn is_focused(&self, id: ComponentId) -> bool {
        self.current().map_or(false, |c| c == id)
    }

    /// Replace the focus ring contents (e.g., on workspace switch).
    /// Tries to keep the same focused ID if it exists in the new set.
    pub fn set_items(&mut self, items: Vec<ComponentId>) {
        let old = self.current();
        self.items = items;
        // Try to restore focus to same component
        if let Some(id) = old {
            if let Some(pos) = self.items.iter().position(|&x| x == id) {
                self.current = pos;
                return;
            }
        }
        self.current = 0;
    }

    /// Focus the Nth item in the ring (0-indexed). No-op if out of bounds.
    pub fn set_by_position(&mut self, pos: usize) -> Option<ComponentId> {
        if pos < self.items.len() {
            self.current = pos;
            self.current()
        } else {
            None
        }
    }
}

impl Default for FocusRing {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}
