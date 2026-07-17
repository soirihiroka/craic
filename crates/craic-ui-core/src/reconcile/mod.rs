//! Keyed source-to-widget reconciliation for UI code.
//!
//! The reconciler owns the previous source snapshot and widget handles. Callers
//! supply mount, update, placement, and removal closures for their UI toolkit.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;

pub mod gtk;

pub struct Element<K, S> {
    key: K,
    state: S,
}

impl<K, S> Element<K, S> {
    pub fn new(key: K, state: S) -> Self {
        Self { key, state }
    }

    pub fn key(&self) -> &K {
        &self.key
    }

    pub fn state(&self) -> &S {
        &self.state
    }
}

pub trait RenderEquality<S> {
    fn equal(&mut self, previous: &S, next: &S) -> bool;
}

impl<S, F> RenderEquality<S> for F
where
    F: FnMut(&S, &S) -> bool,
{
    fn equal(&mut self, previous: &S, next: &S) -> bool {
        self(previous, next)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PartialEqRenderState;

impl<S> RenderEquality<S> for PartialEqRenderState
where
    S: PartialEq,
{
    fn equal(&mut self, previous: &S, next: &S) -> bool {
        previous == next
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AlwaysEqual;

impl<S> RenderEquality<S> for AlwaysEqual {
    fn equal(&mut self, _previous: &S, _next: &S) -> bool {
        true
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AlwaysDifferent;

impl<S> RenderEquality<S> for AlwaysDifferent {
    fn equal(&mut self, _previous: &S, _next: &S) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Insert,
    Move,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReconcileStats {
    pub inserted: usize,
    pub updated: usize,
    pub moved: usize,
    pub removed: usize,
    pub unchanged: usize,
}

impl ReconcileStats {
    pub fn changed(self) -> bool {
        self.inserted != 0 || self.updated != 0 || self.moved != 0 || self.removed != 0
    }
}

pub struct Reconciler<K, S, N> {
    entries: Vec<Entry<K, S, N>>,
}

impl<K, S, N> Default for Reconciler<K, S, N> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl<K, S, N> Reconciler<K, S, N> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl<K, S, N> Reconciler<K, S, N>
where
    K: Clone + Eq + Hash,
{
    pub fn reconcile<E, M, U, P, R>(
        &mut self,
        elements: impl IntoIterator<Item = Element<K, S>>,
        mut equality: E,
        mut mount: M,
        mut update: U,
        mut place: P,
        mut remove: R,
    ) -> ReconcileStats
    where
        E: RenderEquality<S>,
        M: FnMut(usize, &K, &S) -> N,
        U: FnMut(usize, &N, &S, &S),
        P: FnMut(usize, &N, &S, Option<&N>, Placement),
        R: FnMut(N),
    {
        let elements = elements.into_iter().collect::<Vec<_>>();
        let next_keys = elements
            .iter()
            .map(|element| element.key.clone())
            .collect::<HashSet<_>>();

        let old_entries = std::mem::take(&mut self.entries);
        let mut old_entries = old_entries.into_iter().map(Some).collect::<Vec<_>>();
        let mut stats = ReconcileStats::default();

        for entry in &mut old_entries {
            let should_remove = entry
                .as_ref()
                .is_some_and(|entry| !next_keys.contains(&entry.key));
            if should_remove {
                let entry = entry.take().expect("entry exists");
                remove(entry.node);
                stats.removed += 1;
            }
        }

        let mut old_by_key = HashMap::new();
        for (index, entry) in old_entries.iter().enumerate() {
            let Some(entry) = entry else {
                continue;
            };
            old_by_key.entry(entry.key.clone()).or_insert(index);
        }

        let mut retained_previous = HashMap::new();
        let mut previous_retained_key: Option<K> = None;
        for entry in old_entries.iter().flatten() {
            if next_keys.contains(&entry.key) {
                retained_previous.insert(entry.key.clone(), previous_retained_key.clone());
                previous_retained_key = Some(entry.key.clone());
            }
        }

        let mut used_old = HashSet::new();
        let mut next_entries = Vec::with_capacity(elements.len());
        let mut previous_key: Option<K> = None;
        let mut layout_dirty = false;

        for (index, element) in elements.into_iter().enumerate() {
            let previous_node = next_entries
                .last()
                .map(|entry: &Entry<K, S, N>| &entry.node);
            let old_index = old_by_key.get(&element.key).copied().filter(|old_index| {
                !used_old.contains(old_index)
                    && old_entries
                        .get(*old_index)
                        .and_then(Option::as_ref)
                        .is_some()
            });

            match old_index {
                Some(old_index) => {
                    used_old.insert(old_index);
                    let mut entry = old_entries[old_index].take().expect("entry exists");
                    if equality.equal(&entry.state, &element.state) {
                        stats.unchanged += 1;
                    } else {
                        update(index, &entry.node, &entry.state, &element.state);
                        stats.updated += 1;
                    }

                    let old_previous = retained_previous.get(&entry.key).cloned().flatten();
                    let moved = old_previous != previous_key;
                    if moved || layout_dirty {
                        place(
                            index,
                            &entry.node,
                            &element.state,
                            previous_node,
                            Placement::Move,
                        );
                        layout_dirty = true;
                    }
                    if moved {
                        stats.moved += 1;
                    }
                    entry.state = element.state;
                    previous_key = Some(entry.key.clone());
                    next_entries.push(entry);
                }
                None => {
                    let node = mount(index, &element.key, &element.state);
                    place(
                        index,
                        &node,
                        &element.state,
                        previous_node,
                        Placement::Insert,
                    );
                    stats.inserted += 1;
                    layout_dirty = true;
                    previous_key = Some(element.key.clone());
                    next_entries.push(Entry {
                        key: element.key,
                        state: element.state,
                        node,
                    });
                }
            }
        }

        for entry in old_entries.into_iter().flatten() {
            remove(entry.node);
            stats.removed += 1;
        }

        self.entries = next_entries;
        stats
    }
}

struct Entry<K, S, N> {
    key: K,
    state: S,
    node: N,
}
