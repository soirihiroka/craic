#[derive(Clone, PartialEq)]
pub(in crate::ui) struct TreeRow<K, S> {
    pub(in crate::ui) key: K,
    pub(in crate::ui) depth: usize,
    pub(in crate::ui) height: f64,
    pub(in crate::ui) branch: bool,
    pub(in crate::ui) expanded: bool,
    pub(in crate::ui) sticky: bool,
    pub(in crate::ui) state: S,
}

#[derive(Clone, PartialEq)]
pub(in crate::ui) struct TreeRenderState<K, S> {
    pub(in crate::ui) row: TreeRow<K, S>,
    pub(in crate::ui) sticky: bool,
    pub(in crate::ui) bottom: bool,
    pub(in crate::ui) y: f64,
    pub(in crate::ui) width: i32,
}

#[derive(Clone)]
pub(in crate::ui) struct StickyLayoutItem<K, S> {
    pub(in crate::ui) row: TreeRow<K, S>,
    pub(in crate::ui) draw_y: f64,
    pub(in crate::ui) visible_bottom: f64,
}

struct StickyLayout<K, S> {
    items: Vec<StickyLayoutItem<K, S>>,
}

pub(in crate::ui) fn sticky_items<K, S>(
    rows: &[TreeRow<K, S>],
    scroll_y: f64,
) -> Vec<StickyLayoutItem<K, S>>
where
    K: Clone,
    S: Clone,
{
    sticky_layout(rows, scroll_y).items
}

fn sticky_layout<K, S>(rows: &[TreeRow<K, S>], scroll_y: f64) -> StickyLayout<K, S>
where
    K: Clone,
    S: Clone,
{
    if scroll_y <= 0.0 {
        return StickyLayout { items: Vec::new() };
    }

    let offsets = row_offsets(rows);
    let mut items = Vec::new();
    let mut sticky_height = 0.0;
    let mut first_visible_y = scroll_y;
    let mut previous_index: Option<usize> = None;

    loop {
        let Some(first_index) = row_index_at_y(rows, &offsets, first_visible_y) else {
            break;
        };
        let Some(candidate_index) = ancestor_under_previous(rows, first_index, previous_index)
        else {
            break;
        };

        if !node_is_uncollapsed_parent(rows, candidate_index) {
            break;
        }
        let candidate = &rows[candidate_index];
        if !candidate.sticky {
            break;
        }

        if candidate_index == first_index
            && node_top_aligns_with_sticky_bottom(
                &offsets,
                candidate_index,
                scroll_y,
                sticky_height,
            )
        {
            break;
        }

        let end_index = subtree_end_index(rows, candidate_index);
        let position = sticky_node_position(
            &offsets,
            rows,
            end_index,
            scroll_y,
            sticky_height,
            candidate.height,
        );
        let visible_bottom = scroll_y + position + candidate.height;
        items.push(StickyLayoutItem {
            row: candidate.clone(),
            draw_y: scroll_y + position,
            visible_bottom: visible_bottom.max(scroll_y),
        });
        sticky_height += candidate.height;
        first_visible_y = scroll_y + position + candidate.height;
        previous_index = Some(candidate_index);
    }

    StickyLayout { items }
}

fn row_offsets<K, S>(rows: &[TreeRow<K, S>]) -> Vec<f64> {
    let mut offsets = Vec::with_capacity(rows.len() + 1);
    let mut current = 0.0;
    offsets.push(current);
    for row in rows {
        current += row.height;
        offsets.push(current);
    }
    offsets
}

fn row_index_at_y<K, S>(rows: &[TreeRow<K, S>], offsets: &[f64], y: f64) -> Option<usize> {
    if y < 0.0 {
        return None;
    }
    let index = offsets
        .partition_point(|offset| *offset <= y)
        .saturating_sub(1);
    (index < rows.len()).then_some(index)
}

fn ancestor_under_previous<K, S>(
    rows: &[TreeRow<K, S>],
    index: usize,
    previous: Option<usize>,
) -> Option<usize> {
    match previous {
        None => ancestor_at_depth(rows, index, 0),
        Some(previous_index) => {
            let previous_row = rows.get(previous_index)?;
            let parent_index = ancestor_at_depth(rows, index, previous_row.depth)?;
            if parent_index != previous_index {
                return None;
            }
            ancestor_at_depth(rows, index, previous_row.depth + 1)
        }
    }
}

fn ancestor_at_depth<K, S>(rows: &[TreeRow<K, S>], index: usize, depth: usize) -> Option<usize> {
    if rows.get(index)?.depth < depth {
        return None;
    }

    for current in (0..=index).rev() {
        let current_depth = rows[current].depth;
        if current_depth == depth {
            return Some(current);
        }
        if current_depth < depth {
            return None;
        }
    }
    None
}

fn node_is_uncollapsed_parent<K, S>(rows: &[TreeRow<K, S>], index: usize) -> bool {
    let Some(row) = rows.get(index) else {
        return false;
    };
    row.branch && row.expanded && subtree_end_index(rows, index) > index
}

fn node_top_aligns_with_sticky_bottom(
    offsets: &[f64],
    index: usize,
    scroll_y: f64,
    sticky_height: f64,
) -> bool {
    let row_top = offsets.get(index).copied().unwrap_or_default();
    (scroll_y - (row_top - sticky_height)).abs() < 0.5
}

fn sticky_node_position<K, S>(
    offsets: &[f64],
    rows: &[TreeRow<K, S>],
    end_index: usize,
    scroll_y: f64,
    sticky_row_top: f64,
    sticky_row_height: f64,
) -> f64 {
    let descendant_bottom = offsets
        .get(end_index + 1)
        .copied()
        .unwrap_or_else(|| rows.iter().map(|row| row.height).sum::<f64>())
        - scroll_y;
    if sticky_row_top + sticky_row_height > descendant_bottom && sticky_row_top <= descendant_bottom
    {
        descendant_bottom - sticky_row_height
    } else {
        sticky_row_top
    }
}

fn subtree_end_index<K, S>(rows: &[TreeRow<K, S>], index: usize) -> usize {
    let Some(row) = rows.get(index) else {
        return index;
    };
    let boundary = rows[index + 1..]
        .iter()
        .position(|next| next.depth <= row.depth)
        .map_or(rows.len(), |offset| index + 1 + offset);
    boundary.saturating_sub(1)
}
