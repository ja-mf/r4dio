//! Generic scrollable + filterable list widget.

use std::cmp::Ordering;

pub struct ScrollableList<T> {
    pub items: Vec<T>,
    pub filtered_indices: Vec<usize>,
    pub selected: usize,
    pub scroll_offset: usize,
    pub filter: String,
    filter_fn: Box<dyn Fn(&T, &str) -> bool + Send + Sync>,
    sort_key: Option<SortKey>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SortKey {
    Default,
    Name,
    Network,
    Location,
    Stars,
    Recent,
    StarsRecent,
    RecentStars,
    Added,
}

impl<T> ScrollableList<T> {
    pub fn new(filter_fn: impl Fn(&T, &str) -> bool + Send + Sync + 'static) -> Self {
        Self {
            items: Vec::new(),
            filtered_indices: Vec::new(),
            selected: 0,
            scroll_offset: 0,
            filter: String::new(),
            filter_fn: Box::new(filter_fn),
            sort_key: None,
        }
    }

    pub fn set_items(&mut self, items: Vec<T>) {
        self.items = items;
        self.rebuild_filter();
    }

    pub fn set_filter(&mut self, query: &str) {
        self.filter = query.to_string();
        let old_idx = self.filtered_indices.get(self.selected).copied();
        self.rebuild_filter();
        // Try to keep the same item selected after filter change
        if let Some(prev) = old_idx {
            if let Some(pos) = self.filtered_indices.iter().position(|&i| i == prev) {
                self.selected = pos;
            } else {
                self.selected = 0;
            }
        }
        self.scroll_offset = 0;
    }

    pub fn rebuild_filter(&mut self) {
        if self.filter.is_empty() {
            self.filtered_indices = (0..self.items.len()).collect();
        } else {
            self.filtered_indices = self
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| (self.filter_fn)(item, &self.filter))
                .map(|(i, _)| i)
                .collect();
        }
        if self.selected >= self.filtered_indices.len() {
            self.selected = self.filtered_indices.len().saturating_sub(1);
        }
    }

    pub fn select_up(&mut self, n: usize) {
        if self.filtered_indices.is_empty() {
            return;
        }
        self.selected = self.selected.saturating_sub(n);
    }

    pub fn select_down(&mut self, n: usize) {
        if self.filtered_indices.is_empty() {
            return;
        }
        self.selected = (self.selected + n).min(self.filtered_indices.len().saturating_sub(1));
    }

    pub fn select_first(&mut self) {
        self.selected = 0;
        self.scroll_offset = 0;
    }

    pub fn select_last(&mut self) {
        self.selected = self.filtered_indices.len().saturating_sub(1);
    }

    pub fn selected_item(&self) -> Option<&T> {
        let idx = self.filtered_indices.get(self.selected)?;
        self.items.get(*idx)
    }

    pub fn selected_original_index(&self) -> Option<usize> {
        self.filtered_indices.get(self.selected).copied()
    }

    /// Returns (original_index, &item) pairs visible in `height` rows.
    /// Call ensure_visible first to update scroll_offset.
    pub fn visible_items(&self, height: usize) -> Vec<(usize, &T)> {
        if height == 0 || self.filtered_indices.is_empty() {
            return Vec::new();
        }
        let end = (self.scroll_offset + height).min(self.filtered_indices.len());
        self.filtered_indices[self.scroll_offset..end]
            .iter()
            .map(|&i| (i, &self.items[i]))
            .collect()
    }

    pub fn ensure_visible(&mut self, height: usize) {
        if height == 0 {
            return;
        }
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + height {
            self.scroll_offset = self.selected.saturating_sub(height - 1);
        }
    }

    /// Handle a click at `row` within the rendered area.
    /// Returns true if selection changed.
    pub fn handle_click(&mut self, row: usize) -> bool {
        let target = self.scroll_offset + row;
        if target < self.filtered_indices.len() {
            self.selected = target;
            return true;
        }
        false
    }

    pub fn len(&self) -> usize {
        self.filtered_indices.len()
    }

    pub fn is_empty(&self) -> bool {
        self.filtered_indices.is_empty()
    }

    pub fn total_len(&self) -> usize {
        self.items.len()
    }

    pub fn selected_in_view(&self, height: usize) -> usize {
        self.selected
            .saturating_sub(self.scroll_offset)
            .min(height.saturating_sub(1))
    }

    /// Set selection by original item index (not filtered index).
    pub fn set_selected_by_original(&mut self, orig_idx: usize) {
        if let Some(pos) = self.filtered_indices.iter().position(|&i| i == orig_idx) {
            self.selected = pos;
        }
    }

    /// Sort items by a custom comparison function.
    /// Note: this reorders `items` in place and rebuilds filtered_indices.
    /// The sort_by_key parameter is used to sort indices without moving items.
    pub fn sort_by<F>(&mut self, mut cmp: F)
    where
        F: FnMut(&T, &T) -> Ordering,
    {
        // Sort filtered_indices by the comparison of items they point to
        self.filtered_indices
            .sort_by(|&a, &b| cmp(&self.items[a], &self.items[b]));
        // selected stays the same position in filtered_indices
    }
}
