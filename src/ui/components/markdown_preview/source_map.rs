use gtk::prelude::*;
use std::ops::Range;

#[derive(Clone, Debug)]
pub(in crate::ui) struct MarkdownPreviewSourceAnchor {
    pub(in crate::ui) source: Range<usize>,
    pub(in crate::ui) y: f64,
    pub(in crate::ui) height: f64,
}

#[derive(Clone)]
pub(super) struct RenderedSourceAnchor {
    pub(super) widget: gtk::Widget,
    pub(super) source: Range<usize>,
}

pub(super) fn measure_source_anchors(
    document: &gtk::Box,
    anchors: &[RenderedSourceAnchor],
) -> Vec<MarkdownPreviewSourceAnchor> {
    let mut measured = anchors
        .iter()
        .filter_map(|anchor| {
            let (_, y) = anchor.widget.translate_coordinates(document, 0.0, 0.0)?;
            Some(MarkdownPreviewSourceAnchor {
                source: anchor.source.clone(),
                y,
                height: f64::from(anchor.widget.allocated_height()).max(1.0),
            })
        })
        .collect::<Vec<_>>();

    measured.sort_by(|left, right| {
        left.y
            .total_cmp(&right.y)
            .then_with(|| left.source.start.cmp(&right.source.start))
            .then_with(|| left.source.end.cmp(&right.source.end))
    });
    measured
}

pub(super) fn source_offset_for_y(
    anchors: &[MarkdownPreviewSourceAnchor],
    y: f64,
) -> Option<usize> {
    let first = anchors.first()?;
    if y <= first.y {
        return Some(first.source.start);
    }

    for pair in anchors.windows(2) {
        let current = &pair[0];
        let next = &pair[1];
        let current_bottom = current.y + current.height.max(1.0);
        if y <= current_bottom {
            let visual_span = current.height.max(1.0);
            let progress = ((y - current.y) / visual_span).clamp(0.0, 1.0);
            let source_span = current
                .source
                .end
                .saturating_sub(current.source.start)
                .max(1);
            return Some(current.source.start + (source_span as f64 * progress).round() as usize);
        }

        if y > next.y {
            continue;
        }

        let visual_span = (next.y - current.y).max(1.0);
        let progress = ((y - current.y) / visual_span).clamp(0.0, 1.0);
        let source_span = next
            .source
            .start
            .saturating_sub(current.source.start)
            .max(1);
        return Some(current.source.start + (source_span as f64 * progress).round() as usize);
    }

    anchors.last().map(|anchor| anchor.source.start)
}

pub(super) fn y_for_source_offset(
    anchors: &[MarkdownPreviewSourceAnchor],
    source_offset: usize,
) -> Option<f64> {
    let first = anchors.first()?;
    if source_offset <= first.source.start {
        return Some(first.y);
    }

    for anchor in anchors {
        if source_offset < anchor.source.start || source_offset > anchor.source.end {
            continue;
        }

        let source_span = anchor.source.end.saturating_sub(anchor.source.start).max(1);
        let progress = (source_offset.saturating_sub(anchor.source.start) as f64
            / source_span as f64)
            .clamp(0.0, 1.0);
        return Some(anchor.y + anchor.height.max(1.0) * progress);
    }

    for pair in anchors.windows(2) {
        let current = &pair[0];
        let next = &pair[1];
        if source_offset > next.source.start {
            continue;
        }

        let source_span = next
            .source
            .start
            .saturating_sub(current.source.start)
            .max(1);
        let progress = (source_offset.saturating_sub(current.source.start) as f64
            / source_span as f64)
            .clamp(0.0, 1.0);
        return Some(current.y + (next.y - current.y) * progress);
    }

    anchors.last().map(|anchor| anchor.y)
}
