use super::{Element, Placement, ReconcileStats, Reconciler, RenderEquality};
use gtk::prelude::*;
use std::cell::RefCell;
use std::hash::Hash;

pub struct BoxReconciler<K, S> {
    inner: Reconciler<K, S, gtk::Widget>,
}

impl<K, S> Default for BoxReconciler<K, S> {
    fn default() -> Self {
        Self {
            inner: Reconciler::new(),
        }
    }
}

impl<K, S> BoxReconciler<K, S> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl<K, S> BoxReconciler<K, S>
where
    K: Clone + Eq + Hash,
{
    pub fn reconcile<E, M, U>(
        &mut self,
        container: &gtk::Box,
        elements: impl IntoIterator<Item = Element<K, S>>,
        equality: E,
        mount: M,
        update: U,
    ) -> ReconcileStats
    where
        E: RenderEquality<S>,
        M: FnMut(usize, &K, &S) -> gtk::Widget,
        U: FnMut(usize, &gtk::Widget, &S, &S),
    {
        self.inner.reconcile(
            elements,
            equality,
            mount,
            update,
            |_, widget, _, previous, placement| match placement {
                Placement::Insert => container.insert_child_after(widget, previous),
                Placement::Move => container.reorder_child_after(widget, previous),
            },
            |widget| container.remove(&widget),
        )
    }
}

pub struct ListBoxReconciler<K, S> {
    inner: Reconciler<K, S, gtk::Widget>,
}

impl<K, S> Default for ListBoxReconciler<K, S> {
    fn default() -> Self {
        Self {
            inner: Reconciler::new(),
        }
    }
}

impl<K, S> ListBoxReconciler<K, S> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl<K, S> ListBoxReconciler<K, S>
where
    K: Clone + Eq + Hash,
{
    pub fn reconcile<E, M, U>(
        &mut self,
        container: &gtk::ListBox,
        elements: impl IntoIterator<Item = Element<K, S>>,
        equality: E,
        mount: M,
        update: U,
    ) -> ReconcileStats
    where
        E: RenderEquality<S>,
        M: FnMut(usize, &K, &S) -> gtk::Widget,
        U: FnMut(usize, &gtk::Widget, &S, &S),
    {
        self.inner.reconcile(
            elements,
            equality,
            mount,
            update,
            |_, widget, _, previous, placement| match placement {
                Placement::Insert => insert_list_box_child(container, widget, previous),
                Placement::Move => reorder_list_box_child(container, widget, previous),
            },
            |widget| container.remove(&widget),
        )
    }
}

pub struct FixedReconciler<K, S> {
    inner: Reconciler<K, S, gtk::Widget>,
}

impl<K, S> Default for FixedReconciler<K, S> {
    fn default() -> Self {
        Self {
            inner: Reconciler::new(),
        }
    }
}

impl<K, S> FixedReconciler<K, S> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl<K, S> FixedReconciler<K, S>
where
    K: Clone + Eq + Hash,
{
    pub fn reconcile<E, L, M, U>(
        &mut self,
        container: &gtk::Fixed,
        elements: impl IntoIterator<Item = Element<K, S>>,
        equality: E,
        position: L,
        mount: M,
        mut update: U,
    ) -> ReconcileStats
    where
        E: RenderEquality<S>,
        L: FnMut(&S) -> (f64, f64),
        M: FnMut(usize, &K, &S) -> gtk::Widget,
        U: FnMut(usize, &gtk::Widget, &S, &S),
    {
        let position = RefCell::new(position);
        self.inner.reconcile(
            elements,
            equality,
            mount,
            |index, widget, previous, next| {
                update(index, widget, previous, next);
                let (previous_x, previous_y) = position.borrow_mut()(previous);
                let (next_x, next_y) = position.borrow_mut()(next);
                if previous_x != next_x || previous_y != next_y {
                    container.move_(widget, next_x, next_y);
                }
            },
            |_, widget, state, previous, placement| {
                let (x, y) = position.borrow_mut()(state);
                match placement {
                    Placement::Insert => {
                        container.put(widget, x, y);
                        reorder_fixed_child(container, widget, previous);
                    }
                    Placement::Move => {
                        reorder_fixed_child(container, widget, previous);
                        container.move_(widget, x, y);
                    }
                }
            },
            |widget| container.remove(&widget),
        )
    }
}

fn reorder_fixed_child(
    container: &gtk::Fixed,
    widget: &gtk::Widget,
    previous: Option<&gtk::Widget>,
) {
    match previous {
        Some(previous) => widget.insert_after(container, Some(previous)),
        None => widget.insert_after(container, gtk::Widget::NONE),
    }
}

fn insert_list_box_child(
    container: &gtk::ListBox,
    widget: &gtk::Widget,
    previous: Option<&gtk::Widget>,
) {
    let position = previous
        .and_then(list_box_row_for_widget)
        .map_or(0, |row| row.index() + 1);
    container.insert(widget, position);
}

fn reorder_list_box_child(
    container: &gtk::ListBox,
    widget: &gtk::Widget,
    previous: Option<&gtk::Widget>,
) {
    let Some(row) = list_box_row_for_widget(widget) else {
        return;
    };
    container.remove(&row);
    insert_list_box_child(container, row.upcast_ref(), previous);
}

fn list_box_row_for_widget(widget: &gtk::Widget) -> Option<gtk::ListBoxRow> {
    if let Ok(row) = widget.clone().downcast::<gtk::ListBoxRow>() {
        return Some(row);
    }

    widget
        .ancestor(gtk::ListBoxRow::static_type())
        .and_then(|widget| widget.downcast::<gtk::ListBoxRow>().ok())
}
