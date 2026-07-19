use crate::git::{BytesComparison, FileComparison};
use crate::ui::file_type::PreviewKind;
use std::cell::RefCell;

#[derive(Default)]
pub struct RightPreviewTracker {
    state: Option<RightPreviewState>,
}

impl RightPreviewTracker {
    pub fn new() -> Self {
        Self::default()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RightPreviewState {
    Home,
    Loading {
        file_path: String,
    },
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

pub fn diff_state(file_path: &str, comparison: &FileComparison) -> RightPreviewState {
    RightPreviewState::Diff {
        file_path: file_path.to_string(),
        row_count: comparison.rows.len(),
        fingerprint: comparison.fingerprint,
    }
}

pub fn binary_state(file_path: &str, comparison: &BytesComparison) -> RightPreviewState {
    let kind = crate::ui::file_type::preview_kind_for_path(file_path, false);
    RightPreviewState::Binary {
        file_path: file_path.to_string(),
        kind,
        fingerprint: comparison.fingerprint,
    }
}

pub fn unavailable_state(file_path: &str, message: &str) -> RightPreviewState {
    RightPreviewState::Unavailable {
        file_path: file_path.to_string(),
        message: message.to_string(),
    }
}

pub fn should_apply_preview(
    tracker: &RefCell<RightPreviewTracker>,
    state: RightPreviewState,
    log_target: &str,
) -> bool {
    let mut tracker = tracker.borrow_mut();
    if tracker.state.as_ref() == Some(&state) {
        return false;
    }
    log_preview_state(log_target, &state);
    tracker.state = Some(state);
    true
}

fn log_preview_state(log_target: &str, state: &RightPreviewState) {
    match state {
        RightPreviewState::Home => {
            log::debug!("{log_target} preview apply state=home");
        }
        RightPreviewState::Loading { file_path } => {
            log::debug!(
                "{log_target} preview apply state=loading path={}",
                file_path
            );
        }
        RightPreviewState::Diff {
            file_path,
            row_count,
            fingerprint,
        } => {
            log::debug!(
                "{log_target} preview apply state=diff path={} rows={} fingerprint={:016x}",
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
                "{log_target} preview apply state=binary path={} kind={:?} fingerprint={:016x}",
                file_path,
                kind,
                fingerprint
            );
        }
        RightPreviewState::Unavailable { file_path, message } => {
            log::debug!(
                "{log_target} preview apply state=unavailable path={} message_bytes={}",
                file_path,
                message.len()
            );
        }
    }
}
