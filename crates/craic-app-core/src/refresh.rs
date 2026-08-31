use crate::{Generation, PageId};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Default)]
pub struct PageRefreshCoordinator {
    generations: BTreeMap<PageId, Generation>,
    refreshing: BTreeSet<PageId>,
}

impl PageRefreshCoordinator {
    pub fn new(pages: impl IntoIterator<Item = PageId>) -> Self {
        Self {
            generations: pages
                .into_iter()
                .map(|page| (page, Generation::INITIAL))
                .collect(),
            refreshing: BTreeSet::new(),
        }
    }

    pub fn begin(&mut self, page: &PageId) -> Generation {
        let generation = self
            .generations
            .get(page)
            .copied()
            .unwrap_or(Generation::INITIAL)
            .next();
        self.generations.insert(page.clone(), generation);
        self.refreshing.insert(page.clone());
        generation
    }

    pub fn is_current(&self, page: &PageId, generation: Generation) -> bool {
        self.generations.get(page).copied() == Some(generation) && self.refreshing.contains(page)
    }

    pub(crate) fn is_refreshing(&self, page: &PageId) -> bool {
        self.refreshing.contains(page)
    }

    pub fn finish(&mut self, page: &PageId, generation: Generation) -> bool {
        if !self.is_current(page, generation) {
            return false;
        }
        self.refreshing.remove(page);
        true
    }

    pub fn cancel(&mut self, page: &PageId) -> bool {
        let was_refreshing = self.refreshing.remove(page);
        let generation = self
            .generations
            .get(page)
            .copied()
            .unwrap_or(Generation::INITIAL)
            .next();
        self.generations.insert(page.clone(), generation);
        was_refreshing
    }

    pub fn cancel_all(&mut self) -> Vec<PageId> {
        let pages = self.refreshing.iter().cloned().collect::<Vec<_>>();
        for page in &pages {
            self.cancel(page);
        }
        pages
    }
}
