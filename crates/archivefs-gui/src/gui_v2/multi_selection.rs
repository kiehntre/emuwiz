//! Small, bounded selection model for explicit Library actions.
use std::collections::BTreeSet;

#[derive(Default)]
pub(super) struct MultiSelection {
    ids: BTreeSet<i64>,
    limit: usize,
}

impl MultiSelection {
    pub fn with_limit(limit: usize) -> Self {
        Self {
            ids: BTreeSet::new(),
            limit,
        }
    }
    pub fn len(&self) -> usize {
        self.ids.len()
    }
    pub fn contains(&self, id: i64) -> bool {
        self.ids.contains(&id)
    }
    pub fn ids(&self) -> impl Iterator<Item = i64> + '_ {
        self.ids.iter().copied()
    }
    pub fn clear(&mut self) {
        self.ids.clear();
    }
    pub fn retain(&mut self, valid: impl Fn(i64) -> bool) {
        self.ids.retain(|id| valid(*id));
    }
    pub fn toggle(&mut self, id: i64) -> Result<bool, usize> {
        if self.ids.remove(&id) {
            return Ok(false);
        }
        if self.ids.len() >= self.limit {
            return Err(self.limit);
        }
        self.ids.insert(id);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_toggle_and_limit() {
        let mut selection = MultiSelection::with_limit(25);
        for id in 0..25 {
            assert_eq!(selection.toggle(id), Ok(true));
        }
        assert_eq!(selection.toggle(25), Err(25));
        assert_eq!(selection.toggle(3), Ok(false));
        assert_eq!(selection.toggle(25), Ok(true));
        selection.retain(|id| id != 25);
        assert!(!selection.contains(25));
    }
}
