use super::super::canvas_overshoot;
use super::selection::{
    AnchoredSelection, DragSelection, drag_for_mode, selection_for_drag, selection_for_mode,
    word_bounds_at,
};
use super::{
    CompletionUi, EditorState, FoldControlKey, FoldRange, HistorySnapshot, MAX_HISTORY_SNAPSHOTS,
    Selection, SelectionMode, notify_edit, render, selection_bounds,
};
use crate::config;
use crate::language_support::{CompletionSet, NewlineContext, enter_newline};
use crate::markdown_lint::{MarkdownLintEdit, MarkdownLintIssue};
use crate::spellcheck::SpellcheckIssue;
use crate::ui::components::context_menu::{
    self, MenuActionState, TextContextAction, TextContextMenuState,
};
use crate::ui::{canvas_scroll, canvas_scrollbar};
use adw::prelude::*;
use craic_render_skia::{EditorSelection as SharedEditorSelection, toggle_editor_line_comment};
use gtk::gdk;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;
use unicode_segmentation::UnicodeSegmentation;

const INDENT_TEXT: &str = "    ";

include!("input/interactions.rs");
include!("input/selection.rs");
include!("input/completion.rs");
include!("input/editing.rs");
