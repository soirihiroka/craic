use crate::git::{BytesComparison, FileComparison};
use crate::ui::file_type::PreviewKind;
use craic_diff_ui::{Element, PartialEqRenderState, ReconcileStats};
use std::cell::RefCell;

pub(super) type RightPreviewReconciler =
    craic_diff_ui::Reconciler<&'static str, RightPreviewState, ()>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum RightPreviewState {
    Home,
    Diff {
        file_path: String,
        row_count: usize,
        fingerprint: u64,
    },
    Binary {
        file_path: String,
        kind: PreviewKind,
        fingerprint: u64,
    },
    Unavailable {
        file_path: String,
        message: String,
    },
}

pub(super) fn diff_state(file_path: &str, comparison: &FileComparison) -> RightPreviewState {
    RightPreviewState::Diff {
        file_path: file_path.to_string(),
        row_count: comparison.rows.len(),
        fingerprint: comparison.fingerprint,
    }
}

pub(super) fn binary_state(file_path: &str, comparison: &BytesComparison) -> RightPreviewState {
    let kind = crate::ui::file_type::preview_kind_for_path(file_path, false);
    RightPreviewState::Binary {
        file_path: file_path.to_string(),
        kind,
        fingerprint: comparison.fingerprint,
    }
}

pub(super) fn unavailable_state(file_path: &str, message: &str) -> RightPreviewState {
    RightPreviewState::Unavailable {
        file_path: file_path.to_string(),
        message: message.to_string(),
    }
}

pub(super) fn should_update_preview(
    reconciler: &RefCell<RightPreviewReconciler>,
    state: RightPreviewState,
    log_target: &str,
) -> bool {
    let stats = reconciler.borrow_mut().reconcile(
        [Element::new("right", state.clone())],
        PartialEqRenderState,
        |_, _, _| (),
        |_, _, _, _| (),
        |_, _, _, _, _| {},
        |_| {},
    );
    log_preview_stats(log_target, &state, stats);
    stats.changed()
}

fn log_preview_stats(log_target: &str, state: &RightPreviewState, stats: ReconcileStats) {
    match state {
        RightPreviewState::Home => {
            log::debug!("{log_target} preview reconcile state=home stats={stats:?}");
        }
        RightPreviewState::Diff {
            file_path,
            row_count,
            fingerprint,
        } => {
            log::debug!(
                "{log_target} preview reconcile state=diff path={} rows={} fingerprint={:016x} stats={stats:?}",
                file_path,
                row_count,
                fingerprint
            );
        }
        RightPreviewState::Binary {
            file_path,
            kind,
            fingerprint,
        } => {
            log::debug!(
                "{log_target} preview reconcile state=binary path={} kind={:?} fingerprint={:016x} stats={stats:?}",
                file_path,
                kind,
                fingerprint
            );
        }
        RightPreviewState::Unavailable { file_path, message } => {
            log::debug!(
                "{log_target} preview reconcile state=unavailable path={} message_bytes={} stats={stats:?}",
                file_path,
                message.len()
            );
        }
    }
}
