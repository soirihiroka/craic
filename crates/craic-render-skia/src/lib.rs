mod canvas;
mod code_text;
mod diff;
mod diff_layout;
mod diff_painter;
mod editor;
mod editor_buffer;
mod editor_painter;
mod editor_search;
mod editor_selection;
mod geometry;
mod scrollbar;
mod sixel;
mod skia_canvas;
mod syntax;
mod terminal;
mod terminal_painter;

pub use canvas::CanvasPainter;
pub use code_text::CodeTextPaintCache;
pub use diff::{
    DiffDocument, DiffFoldRange, DiffRow, DiffRowKind, DiffSearchMatch, DiffSide, DiffTextPoint,
    DiffTextSelection, build_initial_diff_folds, diff_text_for_side, display_diff_rows,
    find_diff_search_matches, normalize_diff_folds, select_all_diff_text, selected_diff_text,
};
pub use diff_layout::{
    DiffLayoutCache, DiffLayoutRequest, DiffLayoutSignature, DiffMarkerKind, DiffRowLayout,
    DiffScrollbarMarker, DiffWrappedLine, build_diff_layout, diff_row_index_at_y,
    visible_diff_row_range,
};
pub use diff_painter::{DiffPaintRequest, paint_diff};
pub use editor::{
    EditorDocument, EditorFoldRange, EditorLine, EditorLineCommentEdit, EditorMetrics,
    EditorSelection, EditorViewport, selected_editor_text, toggle_editor_line_comment,
};
pub use editor_buffer::{
    EditorTextBuffer, byte_offset_for_line_column, clamp_to_char_boundary, next_char_boundary,
    next_word_boundary, previous_char_boundary, previous_word_boundary,
};
pub use editor_painter::{EditorPaintRequest, paint_editor};
pub use editor_search::{EditorSearchMatch, editor_search_index_after, find_editor_search_matches};
pub use editor_selection::{
    AnchoredSelection, CodeSelection, DragSelection, SelectionMode, clipped_bounds, drag_for_mode,
    ordered_bounds, selection_for_drag, selection_for_mode, word_bounds_at,
};
pub use geometry::{Point, Rect, ScrollGeometry, SelectionRange};
pub use scrollbar::{
    VERTICAL_SCROLLBAR_MIN_THUMB, VERTICAL_SCROLLBAR_VERTICAL_MARGIN, VERTICAL_SCROLLBAR_WIDTH,
    VerticalScrollbarLayout, vertical_scrollbar_handle_rect, vertical_scrollbar_layout,
    vertical_scrollbar_scroll_for_delta, vertical_scrollbar_scroll_for_drag,
    vertical_scrollbar_scroll_for_press, vertical_scrollbar_track_rect,
};
pub use skia_canvas::Context;
pub use syntax::{
    DiffSyntaxSpan, TextDiagnosticKind, TextDiagnosticSpan, TextSyntaxAnalysis, TextSyntaxSpan,
    analyze_text_syntax, build_diff_syntax, build_text_syntax,
};
pub use terminal::{
    TerminalCell, TerminalCellStyle, TerminalClipboard, TerminalColor, TerminalCursor,
    TerminalCursorShape, TerminalEventBatch, TerminalImage, TerminalMouseAction,
    TerminalMouseButton, TerminalMouseModifiers, TerminalScroll, TerminalSearchDirection,
    TerminalSearchMatch, TerminalSelectionType, TerminalSession, TerminalSide, TerminalSnapshot,
    TerminalSpawnOptions, TerminalViewport,
};
pub use terminal_painter::{TerminalPaintCache, TerminalPaintRequest, paint_terminal};
