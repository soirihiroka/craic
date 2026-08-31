use crate::application::AppDelegate;
use craic_language::{language_id_from_path, language_support_for_id};
use craic_render_skia::{
    DragSelection, EditorDocument, EditorFoldRange, EditorMetrics, EditorPaintRequest,
    EditorSearchMatch, EditorSelection, EditorViewport, SelectionMode, TextDiagnosticSpan,
    TextSyntaxSpan, drag_for_mode, editor_search_index_after, find_editor_search_matches,
    next_word_boundary, paint_editor, previous_word_boundary, selected_editor_text,
    selection_for_drag, toggle_editor_line_comment, word_bounds_at,
};
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityTextAreaRole, NSAutoresizingMaskOptions, NSCursor, NSEvent,
    NSEventModifierFlags, NSMenu, NSMenuItem, NSPasteboard, NSPasteboardTypeString,
    NSTextInputClient, NSView,
};
use objc2_core_foundation::CGSize;
use objc2_foundation::{
    MainThreadMarker, NSArray, NSAttributedString, NSAttributedStringKey, NSNotFound,
    NSObjectProtocol, NSRange, NSRangePointer, NSRect, NSSize, NSString, NSUInteger,
};
use objc2_metal::{
    MTLCommandBuffer, MTLCommandQueue, MTLCreateSystemDefaultDevice, MTLDevice, MTLDrawable,
    MTLPixelFormat,
};
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};
use skia_safe::gpu::{self, DirectContext, SurfaceOrigin, backend_render_targets, mtl};
use skia_safe::{ColorType, Paint, Rect};
use std::cell::{Cell, RefCell};

const SCROLLBAR_WIDTH: f64 = 12.0;

struct MetalState {
    _device: Retained<ProtocolObject<dyn MTLDevice>>,
    layer: Retained<CAMetalLayer>,
    command_queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    skia: DirectContext,
}

impl Drop for MetalState {
    fn drop(&mut self) {
        log::info!("releasing native Skia Metal editor resources");
        self.skia.release_resources_and_abandon();
    }
}

struct CodeState {
    path: String,
    document: EditorDocument,
    selection: EditorSelection,
    viewport: EditorViewport,
    metrics: EditorMetrics,
    editable: bool,
    marked_text: String,
    diagnostics: Vec<TextDiagnosticSpan>,
    search_query: String,
    search_matches: Vec<EditorSearchMatch>,
    active_search_match: Option<usize>,
    manual_folds: Vec<(usize, usize)>,
    completion_items: Vec<String>,
    completion_selected: usize,
    completion_range: Option<(usize, usize)>,
    undo: Vec<EditorSnapshot>,
    redo: Vec<EditorSnapshot>,
}

struct EditorSnapshot {
    text: String,
    selection: EditorSelection,
}

impl CodeState {
    fn clamp_scroll(&mut self, viewport: NSSize) {
        self.viewport.resize(
            viewport.width,
            viewport.height,
            &self.document,
            self.metrics,
        );
    }

    fn reveal_selection(&mut self, viewport: NSSize) {
        self.viewport.resize(
            viewport.width,
            viewport.height,
            &self.document,
            self.metrics,
        );
        self.viewport.reveal_offset(
            &self.document,
            self.selection.focus,
            self.metrics,
            SCROLLBAR_WIDTH,
        );
    }
}

pub(crate) struct CodeMetalViewIvars {
    metal: RefCell<Option<MetalState>>,
    state: RefCell<CodeState>,
    delegate: RefCell<Weak<AppDelegate>>,
    dragging_selection: Cell<Option<DragSelection<usize>>>,
}

define_class!(
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = CodeMetalViewIvars]
    pub(crate) struct CodeMetalView;

    unsafe impl NSObjectProtocol for CodeMetalView {}

    unsafe impl NSTextInputClient for CodeMetalView {
        #[unsafe(method(insertText:replacementRange:))]
        unsafe fn insert_text_replacement_range(
            &self,
            string: &AnyObject,
            replacement_range: NSRange,
        ) {
            let text = editor_input_text(string);
            self.ivars().state.borrow_mut().marked_text.clear();
            self.replace_text(text, replacement_range);
        }

        #[unsafe(method(doCommandBySelector:))]
        unsafe fn do_command_by_selector(&self, selector: Sel) {
            if selector == sel!(insertNewline:) {
                if !self.accept_completion() {
                    self.replace_text("\n".to_string(), NSRange::new(NSNotFound as usize, 0));
                }
            } else if selector == sel!(insertTab:) {
                if !self.accept_completion() {
                    self.replace_text("\t".to_string(), NSRange::new(NSNotFound as usize, 0));
                }
            } else if selector == sel!(deleteBackward:) {
                self.delete_backward();
            } else if selector == sel!(deleteForward:) {
                self.delete_forward();
            } else if selector == sel!(deleteWordBackward:) {
                self.delete_word(false);
            } else if selector == sel!(deleteWordForward:) {
                self.delete_word(true);
            } else if selector == sel!(moveLeft:) {
                self.move_horizontal(-1, false);
            } else if selector == sel!(moveRight:) {
                self.move_horizontal(1, false);
            } else if selector == sel!(moveLeftAndModifySelection:) {
                self.move_horizontal(-1, true);
            } else if selector == sel!(moveRightAndModifySelection:) {
                self.move_horizontal(1, true);
            } else if selector == sel!(moveWordLeft:) {
                self.move_word(false, false);
            } else if selector == sel!(moveWordRight:) {
                self.move_word(true, false);
            } else if selector == sel!(moveWordLeftAndModifySelection:) {
                self.move_word(false, true);
            } else if selector == sel!(moveWordRightAndModifySelection:) {
                self.move_word(true, true);
            } else if selector == sel!(moveUp:) {
                if !self.select_completion(-1) {
                    self.move_vertical(-1, false);
                }
            } else if selector == sel!(moveDown:) {
                if !self.select_completion(1) {
                    self.move_vertical(1, false);
                }
            } else if selector == sel!(moveUpAndModifySelection:) {
                self.move_vertical(-1, true);
            } else if selector == sel!(moveDownAndModifySelection:) {
                self.move_vertical(1, true);
            } else if selector == sel!(moveToBeginningOfLine:)
                || selector == sel!(moveToLeftEndOfLine:)
            {
                self.move_to_line_edge(false, false);
            } else if selector == sel!(moveToEndOfLine:)
                || selector == sel!(moveToRightEndOfLine:)
            {
                self.move_to_line_edge(true, false);
            } else if selector == sel!(moveToBeginningOfLineAndModifySelection:)
                || selector == sel!(moveToLeftEndOfLineAndModifySelection:)
            {
                self.move_to_line_edge(false, true);
            } else if selector == sel!(moveToEndOfLineAndModifySelection:)
                || selector == sel!(moveToRightEndOfLineAndModifySelection:)
            {
                self.move_to_line_edge(true, true);
            } else if selector == sel!(moveToBeginningOfDocument:) {
                self.move_to_document_edge(false, false);
            } else if selector == sel!(moveToEndOfDocument:) {
                self.move_to_document_edge(true, false);
            } else if selector == sel!(moveToBeginningOfDocumentAndModifySelection:) {
                self.move_to_document_edge(false, true);
            } else if selector == sel!(moveToEndOfDocumentAndModifySelection:) {
                self.move_to_document_edge(true, true);
            } else if selector == sel!(pageUp:) {
                self.page(-1);
            } else if selector == sel!(pageDown:) {
                self.page(1);
            } else if selector == sel!(cancelOperation:) {
                self.ivars().state.borrow_mut().marked_text.clear();
                self.dismiss_completion();
                self.render_frame();
            }
        }

        #[unsafe(method(setMarkedText:selectedRange:replacementRange:))]
        unsafe fn set_marked_text_selected_range_replacement_range(
            &self,
            string: &AnyObject,
            _selected_range: NSRange,
            _replacement_range: NSRange,
        ) {
            self.ivars().state.borrow_mut().marked_text = editor_input_text(string);
            self.render_frame();
        }

        #[unsafe(method(unmarkText))]
        fn unmark_text(&self) {
            let text = std::mem::take(&mut self.ivars().state.borrow_mut().marked_text);
            if !text.is_empty() {
                self.replace_text(text, NSRange::new(NSNotFound as usize, 0));
            }
        }

        #[unsafe(method(selectedRange))]
        fn selected_range(&self) -> NSRange {
            let state = self.ivars().state.borrow();
            selection_utf16(&state.document, state.selection)
        }

        #[unsafe(method(markedRange))]
        fn marked_range(&self) -> NSRange {
            let state = self.ivars().state.borrow();
            if state.marked_text.is_empty() {
                NSRange::new(NSNotFound as usize, 0)
            } else {
                let selection = selection_utf16(&state.document, state.selection);
                NSRange::new(selection.location, state.marked_text.encode_utf16().count())
            }
        }

        #[unsafe(method(hasMarkedText))]
        fn has_marked_text(&self) -> bool {
            !self.ivars().state.borrow().marked_text.is_empty()
        }

        #[unsafe(method_id(attributedSubstringForProposedRange:actualRange:))]
        unsafe fn attributed_substring_for_proposed_range_actual_range(
            &self,
            range: NSRange,
            actual_range: NSRangePointer,
        ) -> Option<Retained<NSAttributedString>> {
            let state = self.ivars().state.borrow();
            let (start, end) = byte_range_for_utf16(state.document.text(), range);
            if !actual_range.is_null() {
                unsafe {
                    *actual_range = utf16_range_for_bytes(state.document.text(), start, end)
                };
            }
            state
                .document
                .text()
                .get(start..end)
                .map(|text| NSAttributedString::from_nsstring(&NSString::from_str(text)))
        }

        #[unsafe(method_id(validAttributesForMarkedText))]
        fn valid_attributes_for_marked_text(&self) -> Retained<NSArray<NSAttributedStringKey>> {
            NSArray::new()
        }

        #[unsafe(method(firstRectForCharacterRange:actualRange:))]
        unsafe fn first_rect_for_character_range_actual_range(
            &self,
            range: NSRange,
            actual_range: NSRangePointer,
        ) -> NSRect {
            if !actual_range.is_null() {
                unsafe { *actual_range = range };
            }
            let state = self.ivars().state.borrow();
            let byte = byte_offset_for_utf16(state.document.text(), range.location);
            let (line, column) = state.document.line_column_for_offset(byte);
            let local = NSRect::new(
                objc2_foundation::NSPoint::new(
                    state.metrics.gutter_width as f64
                        + state.metrics.text_inset as f64
                        + column as f64 * state.metrics.char_width as f64
                        - state.viewport.scroll_x(),
                    state.metrics.text_inset as f64
                        + line as f64 * state.metrics.line_height as f64
                        - state.viewport.scroll_y(),
                ),
                NSSize::new(
                    state.metrics.char_width as f64,
                    state.metrics.line_height as f64,
                ),
            );
            drop(state);
            let window_rect = self.convertRect_toView(local, None);
            self.window()
                .map_or(window_rect, |window| window.convertRectToScreen(window_rect))
        }

        #[unsafe(method(characterIndexForPoint:))]
        fn character_index_for_point(
            &self,
            point: objc2_foundation::NSPoint,
        ) -> NSUInteger {
            let window_point = self
                .window()
                .map_or(point, |window| window.convertPointFromScreen(point));
            let local = self.convertPoint_fromView(window_point, None);
            let state = self.ivars().state.borrow();
            let byte = state.document.hit_test(
                local.x,
                local.y,
                state.viewport.scroll_x(),
                state.viewport.scroll_y(),
                state.metrics,
            );
            utf16_offset_for_byte(state.document.text(), byte)
        }
    }

    impl CodeMetalView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool { true }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool { true }

        #[unsafe(method(mouseDownCanMoveWindow))]
        fn mouse_down_can_move_window(&self) -> bool { false }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) { self.render_frame(); }

        #[unsafe(method(viewDidChangeBackingProperties))]
        fn view_did_change_backing_properties(&self) { self.render_frame(); }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: NSSize) {
            unsafe { let _: () = msg_send![super(self), setFrameSize: size]; }
            self.ivars().state.borrow_mut().clamp_scroll(size);
            self.render_frame();
        }

        #[unsafe(method(resetCursorRects))]
        fn reset_cursor_rects(&self) {
            self.addCursorRect_cursor(self.bounds(), &NSCursor::IBeamCursor());
        }

        #[unsafe(method(becomeFirstResponder))]
        fn become_first_responder(&self) -> bool {
            self.render_frame();
            true
        }

        #[unsafe(method(resignFirstResponder))]
        fn resign_first_responder(&self) -> bool {
            self.ivars().state.borrow_mut().marked_text.clear();
            self.render_frame();
            true
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            let mut state = self.ivars().state.borrow_mut();
            let CodeState {
                document,
                viewport,
                metrics,
                ..
            } = &mut *state;
            viewport.scroll_by(
                -event.scrollingDeltaX(),
                -event.scrollingDeltaY(),
                document,
                *metrics,
            );
            drop(state);
            self.notify_scroll_changed();
            self.update_accessibility_viewport();
            self.render_frame();
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            if let Some(window) = self.window() { window.makeFirstResponder(Some(self)); }
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            if point.x <= 28.0 && self.toggle_fold_at_y(point.y) {
                return;
            }
            let mut state = self.ivars().state.borrow_mut();
            clear_completion_state(&mut state);
            let offset = state.document.hit_test(
                point.x,
                point.y,
                state.viewport.scroll_x(),
                state.viewport.scroll_y(),
                state.metrics,
            );
            let anchor = if event.modifierFlags().contains(NSEventModifierFlags::Shift) {
                state.selection.anchor
            } else {
                offset
            };
            if event.modifierFlags().contains(NSEventModifierFlags::Shift) {
                state.selection = EditorSelection {
                    anchor,
                    focus: offset,
                };
                self.ivars()
                    .dragging_selection
                    .set(Some(DragSelection::Character { anchor }));
            } else {
                let mode = SelectionMode::for_press_count(event.clickCount() as i32);
                let (drag, selection) = drag_for_mode(
                    offset,
                    mode,
                    |point| word_bounds_at(state.document.text(), point),
                    |point| editor_line_bounds(&state.document, point),
                );
                state.selection = EditorSelection {
                    anchor: selection.anchor,
                    focus: selection.focus,
                };
                self.ivars().dragging_selection.set(Some(drag));
            }
            drop(state);
            self.update_accessibility();
            self.render_frame();
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            let Some(drag) = self.ivars().dragging_selection.get() else {
                return;
            };
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            let mut state = self.ivars().state.borrow_mut();
            let CodeState {
                document,
                viewport,
                metrics,
                ..
            } = &mut *state;
            let scrolled = viewport.autoscroll_for_pointer(point.x, point.y, document, *metrics);
            let focus = state.document.hit_test(
                point.x,
                point.y,
                state.viewport.scroll_x(),
                state.viewport.scroll_y(),
                state.metrics,
            );
            let selection = selection_for_drag(
                drag,
                focus,
                |point| word_bounds_at(state.document.text(), point),
                |point| editor_line_bounds(&state.document, point),
            );
            state.selection = EditorSelection {
                anchor: selection.anchor,
                focus: selection.focus,
            };
            drop(state);
            if scrolled {
                self.notify_scroll_changed();
            }
            self.update_accessibility();
            self.render_frame();
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            self.ivars().dragging_selection.set(None);
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            if !self.ivars().state.borrow().editable {
                unsafe { let _: () = msg_send![super(self), keyDown: event]; }
                return;
            }
            let modifiers = event.modifierFlags();
            let unmodified = event
                .charactersIgnoringModifiers()
                .map(|value| value.to_string())
                .unwrap_or_default();
            if modifiers.contains(NSEventModifierFlags::Command) {
                match unmodified.as_str() {
                    "a" => self.select_all_internal(),
                    "c" => self.copy_selection(),
                    "x" => self.cut_selection(),
                    "v" => self.paste_clipboard(),
                    "z" if modifiers.contains(NSEventModifierFlags::Shift) => self.redo(),
                    "z" => self.undo(),
                    "/" if !modifiers.contains(NSEventModifierFlags::Shift) => {
                        self.toggle_line_comment()
                    }
                    _ => unsafe { let _: () = msg_send![super(self), keyDown: event]; },
                }
                return;
            }
            self.interpretKeyEvents(&NSArray::from_slice(&[event]));
        }

        #[unsafe(method_id(menuForEvent:))]
        fn menu_for_event(&self, _event: &NSEvent) -> Option<Retained<NSMenu>> {
            let menu = NSMenu::new(self.mtm());
            for (title, selector, key) in [
                ("Undo", objc2::sel!(undo:), "z"),
                ("Redo", objc2::sel!(redo:), "Z"),
                ("Cut", objc2::sel!(cut:), "x"),
                ("Copy", objc2::sel!(copy:), "c"),
                ("Paste", objc2::sel!(paste:), "v"),
                ("Select All", objc2::sel!(selectAll:), "a"),
            ] {
                let item = unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(self.mtm()), &NSString::from_str(title), Some(selector),
                    &NSString::from_str(key),
                ) };
                unsafe { item.setTarget(Some(self)); }
                menu.addItem(&item);
            }
            menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
            for (title, selector) in [
                ("Fold Selection", objc2::sel!(foldSelection:)),
                ("Unfold All", objc2::sel!(unfoldAll:)),
            ] {
                let item = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(self.mtm()),
                        &NSString::from_str(title),
                        Some(selector),
                        &NSString::new(),
                    )
                };
                unsafe { item.setTarget(Some(self)); }
                menu.addItem(&item);
            }
            Some(menu)
        }

        #[unsafe(method(copy:))]
        fn copy(&self, _sender: &AnyObject) {
            self.copy_selection();
        }

        #[unsafe(method(undo:))]
        fn undo_action(&self, _sender: &AnyObject) { self.undo(); }

        #[unsafe(method(redo:))]
        fn redo_action(&self, _sender: &AnyObject) { self.redo(); }

        #[unsafe(method(cut:))]
        fn cut(&self, sender: &AnyObject) {
            let _ = sender;
            self.cut_selection();
        }

        #[unsafe(method(paste:))]
        fn paste(&self, _sender: &AnyObject) {
            self.paste_clipboard();
        }

        #[unsafe(method(selectAll:))]
        fn select_all(&self, _sender: &AnyObject) {
            self.select_all_internal();
        }

        #[unsafe(method(foldSelection:))]
        fn fold_selection_action(&self, _sender: &AnyObject) {
            self.fold_selection();
        }

        #[unsafe(method(unfoldAll:))]
        fn unfold_all_action(&self, _sender: &AnyObject) {
            self.unfold_all();
        }
    }
);

impl CodeMetalView {
    pub fn new(frame: NSRect, font_size: f64, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(CodeMetalViewIvars {
            metal: RefCell::new(None),
            state: RefCell::new(CodeState {
                path: String::new(),
                document: EditorDocument::default(),
                selection: EditorSelection::default(),
                viewport: EditorViewport::default(),
                metrics: EditorMetrics::for_font_size(font_size as f32),
                editable: false,
                marked_text: String::new(),
                diagnostics: Vec::new(),
                search_query: String::new(),
                search_matches: Vec::new(),
                active_search_match: None,
                manual_folds: Vec::new(),
                completion_items: Vec::new(),
                completion_selected: 0,
                completion_range: None,
                undo: Vec::new(),
                redo: Vec::new(),
            }),
            delegate: RefCell::new(Weak::default()),
            dragging_selection: Cell::new(None),
        });
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        view.initialize_metal();
        view.setAccessibilityElement(true);
        view.setAccessibilityRole(Some(unsafe { NSAccessibilityTextAreaRole }));
        view.setAccessibilityLabel(Some(&NSString::from_str("Code editor")));
        view.update_accessibility();
        view
    }

    pub fn attach_delegate(&self, delegate: &AppDelegate) {
        self.ivars().delegate.replace(Weak::new(delegate));
        log::debug!("native Skia editor attached its weak lifecycle delegate");
    }

    pub fn set_document(
        &self,
        path: &str,
        text: String,
        syntax: Vec<TextSyntaxSpan>,
        selection: EditorSelection,
        editable: bool,
        reset_scroll: bool,
    ) {
        let mut state = self.ivars().state.borrow_mut();
        state.path = path.to_string();
        state.document = EditorDocument::new(text, syntax);
        state.selection = EditorSelection {
            anchor: state.document.clamp_offset(selection.anchor),
            focus: state.document.clamp_offset(selection.focus),
        };
        state.editable = editable;
        state.marked_text.clear();
        state.diagnostics.clear();
        state.search_query.clear();
        state.search_matches.clear();
        state.active_search_match = None;
        state.manual_folds.clear();
        state.completion_items.clear();
        state.completion_selected = 0;
        state.completion_range = None;
        state.undo.clear();
        state.redo.clear();
        if reset_scroll {
            state.viewport.reset();
        }
        state.reveal_selection(self.bounds().size);
        drop(state);
        self.update_accessibility();
        self.render_frame();
    }

    pub fn set_syntax(&self, syntax: Vec<TextSyntaxSpan>) {
        let mut state = self.ivars().state.borrow_mut();
        let text = state.document.text().to_string();
        let folds = state.document.folds().to_vec();
        state.document = EditorDocument::new_with_folds(text, syntax, folds);
        state.selection = EditorSelection {
            anchor: state.document.clamp_offset(state.selection.anchor),
            focus: state.document.clamp_offset(state.selection.focus),
        };
        drop(state);
        self.render_frame();
    }

    pub fn set_folds(&self, ranges: Vec<(usize, usize)>) {
        let mut state = self.ivars().state.borrow_mut();
        let previous = state.document.folds().to_vec();
        let folds = ranges
            .into_iter()
            .chain(state.manual_folds.iter().copied())
            .filter_map(|(start_line, end_line)| {
                (start_line < end_line).then_some(EditorFoldRange {
                    start_line,
                    end_line,
                    expanded: previous
                        .iter()
                        .find(|fold| fold.start_line == start_line && fold.end_line == end_line)
                        .map_or(true, |fold| fold.expanded),
                })
            })
            .collect();
        let text = state.document.text().to_string();
        let syntax = state.document.syntax().to_vec();
        state.document = EditorDocument::new_with_folds(text, syntax, folds);
        state.clamp_scroll(self.bounds().size);
        drop(state);
        self.update_accessibility_viewport();
        self.render_frame();
    }

    pub fn set_diagnostics(&self, mut diagnostics: Vec<TextDiagnosticSpan>) {
        let mut state = self.ivars().state.borrow_mut();
        diagnostics.retain(|diagnostic| {
            diagnostic.start < diagnostic.end
                && diagnostic.end <= state.document.text().len()
                && state.document.text().is_char_boundary(diagnostic.start)
                && state.document.text().is_char_boundary(diagnostic.end)
        });
        diagnostics.sort_by_key(|diagnostic| (diagnostic.start, diagnostic.end));
        state.diagnostics = diagnostics;
        drop(state);
        self.render_frame();
    }

    pub fn set_completions(&self, items: Vec<String>, replacement_range: NSRange) {
        let mut state = self.ivars().state.borrow_mut();
        let range = byte_range_for_utf16(state.document.text(), replacement_range);
        if items.is_empty() || range.1 > state.document.text().len() {
            state.completion_items.clear();
            state.completion_range = None;
        } else {
            state.completion_items = items;
            state.completion_selected = 0;
            state.completion_range = Some(range);
        }
        drop(state);
        self.render_frame();
    }

    pub fn clear_completions(&self) {
        self.dismiss_completion();
    }

    pub fn set_font_size(&self, font_size: f64) {
        let mut state = self.ivars().state.borrow_mut();
        state.metrics = EditorMetrics::for_font_size(font_size as f32);
        state.clamp_scroll(self.bounds().size);
        drop(state);
        self.render_frame();
    }

    pub fn is_focused(&self) -> bool {
        self.window()
            .and_then(|window| window.firstResponder())
            .is_some_and(|responder| {
                Retained::as_ptr(&responder).cast::<AnyObject>()
                    == (self as *const Self).cast::<AnyObject>()
            })
    }

    pub fn set_search_query(&self, query: &str) {
        let mut state = self.ivars().state.borrow_mut();
        state.search_query.clear();
        state.search_query.push_str(query);
        state.search_matches = find_editor_search_matches(state.document.text(), query);
        state.active_search_match =
            editor_search_index_after(&state.search_matches, state.selection.focus);
        activate_search_match(&mut state, self.bounds().size);
        let selection = selection_utf16(&state.document, state.selection);
        drop(state);
        self.notify_selection_changed(selection);
        self.notify_scroll_changed();
        self.update_accessibility();
        self.render_frame();
    }

    pub fn search_next(&self) {
        self.step_search(1);
    }

    pub fn search_previous(&self) {
        self.step_search(-1);
    }

    pub fn search_status(&self) -> (Option<usize>, usize) {
        let state = self.ivars().state.borrow();
        (
            state.active_search_match.map(|index| index + 1),
            state.search_matches.len(),
        )
    }

    pub fn clear_search(&self) {
        let mut state = self.ivars().state.borrow_mut();
        state.search_query.clear();
        state.search_matches.clear();
        state.active_search_match = None;
        drop(state);
        self.render_frame();
    }

    fn step_search(&self, direction: isize) {
        let mut state = self.ivars().state.borrow_mut();
        if state.search_matches.is_empty() {
            return;
        }
        let count = state.search_matches.len();
        state.active_search_match = Some(match (state.active_search_match, direction < 0) {
            (Some(0), true) | (None, true) => count - 1,
            (Some(index), true) => index - 1,
            (Some(index), false) => (index + 1) % count,
            (None, false) => 0,
        });
        activate_search_match(&mut state, self.bounds().size);
        let selection = selection_utf16(&state.document, state.selection);
        drop(state);
        self.notify_selection_changed(selection);
        self.notify_scroll_changed();
        self.update_accessibility();
        self.render_frame();
    }

    fn select_completion(&self, direction: isize) -> bool {
        let mut state = self.ivars().state.borrow_mut();
        let count = state.completion_items.len();
        if count == 0 {
            return false;
        }
        state.completion_selected = if direction < 0 {
            state
                .completion_selected
                .checked_sub(1)
                .unwrap_or(count - 1)
        } else {
            (state.completion_selected + 1) % count
        };
        drop(state);
        self.render_frame();
        true
    }

    fn accept_completion(&self) -> bool {
        let state = self.ivars().state.borrow();
        let Some((start, end)) = state.completion_range else {
            return false;
        };
        let Some(item) = state
            .completion_items
            .get(state.completion_selected)
            .cloned()
        else {
            return false;
        };
        let range = utf16_range_for_bytes(state.document.text(), start, end);
        drop(state);
        self.replace_text(item, range);
        true
    }

    fn dismiss_completion(&self) {
        let mut state = self.ivars().state.borrow_mut();
        if state.completion_items.is_empty() && state.completion_range.is_none() {
            return;
        }
        state.completion_items.clear();
        state.completion_selected = 0;
        state.completion_range = None;
        drop(state);
        self.render_frame();
    }

    pub fn preview_source_offset(&self) -> usize {
        let state = self.ivars().state.borrow();
        state.viewport.source_offset(&state.document, state.metrics)
    }

    pub fn teardown_renderer(&self) {
        self.setLayer(None);
        self.setWantsLayer(false);
        self.ivars().metal.borrow_mut().take();
    }

    fn initialize_metal(&self) {
        let Some(device) = MTLCreateSystemDefaultDevice() else {
            log::error!("Metal is unavailable for native editor");
            return;
        };
        let Some(command_queue) = device.newCommandQueue() else {
            log::error!("Metal command queue creation failed for native editor");
            return;
        };
        let layer = CAMetalLayer::new();
        layer.setDevice(Some(&device));
        layer.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        layer.setFramebufferOnly(false);
        layer.setMaximumDrawableCount(3);
        let backend = unsafe {
            mtl::BackendContext::new(
                Retained::as_ptr(&device) as mtl::Handle,
                Retained::as_ptr(&command_queue) as mtl::Handle,
            )
        };
        let Some(skia) = gpu::direct_contexts::make_metal(&backend, None) else {
            log::error!("Skia Metal context creation failed for native editor");
            return;
        };
        self.setLayer(Some(&layer));
        self.setWantsLayer(true);
        self.ivars().metal.replace(Some(MetalState {
            _device: device,
            layer,
            command_queue,
            skia,
        }));
        log::info!("native Skia Metal code editor surface initialized");
    }

    fn render_frame(&self) {
        if self.window().is_none() || self.isHiddenOrHasHiddenAncestor() {
            return;
        }
        let bounds = self.bounds();
        if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
            return;
        }
        let mut metal_ref = self.ivars().metal.borrow_mut();
        let Some(metal) = metal_ref.as_mut() else {
            return;
        };
        let backing = self.convertRectToBacking(bounds);
        let pixel_width = backing.size.width.max(1.0).round() as i32;
        let pixel_height = backing.size.height.max(1.0).round() as i32;
        let scale_x = pixel_width as f32 / bounds.size.width as f32;
        let scale_y = pixel_height as f32 / bounds.size.height as f32;
        metal.layer.setContentsScale(scale_x as f64);
        metal
            .layer
            .setDrawableSize(CGSize::new(pixel_width as f64, pixel_height as f64));
        let Some(drawable) = metal.layer.nextDrawable() else {
            return;
        };
        let texture = drawable.texture();
        let texture_info =
            unsafe { mtl::TextureInfo::new(Retained::as_ptr(&texture) as mtl::Handle) };
        let target = backend_render_targets::make_mtl((pixel_width, pixel_height), &texture_info);
        let Some(mut surface) = gpu::surfaces::wrap_backend_render_target(
            &mut metal.skia,
            &target,
            SurfaceOrigin::TopLeft,
            ColorType::BGRA8888,
            None,
            None,
        ) else {
            return;
        };
        let canvas = surface.canvas();
        canvas.scale((scale_x, scale_y));
        let state = self.ivars().state.borrow();
        let focused = self
            .window()
            .and_then(|window| window.firstResponder())
            .is_some_and(|responder| {
                Retained::as_ptr(&responder).cast::<AnyObject>()
                    == (self as *const Self).cast::<AnyObject>()
            });
        paint_editor(
            canvas,
            EditorPaintRequest {
                document: &state.document,
                viewport_width: bounds.size.width as f32,
                viewport_height: bounds.size.height as f32,
                scroll_x: state.viewport.scroll_x(),
                scroll_y: state.viewport.scroll_y(),
                selection: state.selection,
                marked_text: (!state.marked_text.is_empty()).then_some(state.marked_text.as_str()),
                diagnostics: &state.diagnostics,
                search_matches: &state.search_matches,
                active_search_match: state.active_search_match,
                completion_items: &state.completion_items,
                completion_selected: state.completion_selected,
                focused,
                metrics: state.metrics,
            },
        );
        self.draw_scrollbar(canvas, &state, bounds.size);
        drop(state);
        metal.skia.flush_and_submit();
        drop(surface);
        let Some(command_buffer) = metal.command_queue.commandBuffer() else {
            return;
        };
        let presentable: &ProtocolObject<dyn MTLDrawable> = drawable.as_ref();
        command_buffer.presentDrawable(presentable);
        command_buffer.commit();
    }

    fn draw_scrollbar(&self, canvas: &skia_safe::Canvas, state: &CodeState, viewport: NSSize) {
        let (_, height) = state.document.content_size(state.metrics);
        let maximum = (height as f64 - viewport.height).max(0.0);
        if maximum <= 0.0 {
            return;
        }
        let track_height = viewport.height.max(1.0);
        let thumb_height = (track_height * viewport.height / height as f64)
            .max(36.0)
            .min(track_height);
        let y = state.viewport.scroll_y() / maximum * (track_height - thumb_height);
        let mut paint = Paint::new(skia_safe::Color4f::new(1.0, 1.0, 1.0, 0.34), None);
        paint.set_anti_alias(true);
        canvas.draw_round_rect(
            Rect::from_xywh(
                (viewport.width - 7.0) as f32,
                (y + 3.0) as f32,
                4.0,
                (thumb_height - 6.0).max(4.0) as f32,
            ),
            2.0,
            2.0,
            &paint,
        );
    }

    fn toggle_fold_at_y(&self, y: f64) -> bool {
        let mut state = self.ivars().state.borrow_mut();
        let visual_line = ((y + state.viewport.scroll_y() - state.metrics.text_inset as f64)
            .max(0.0)
            / state.metrics.line_height as f64)
            .floor() as usize;
        let Some(source_line) = state.document.visual_lines().get(visual_line).copied() else {
            return false;
        };
        let Some(target) = state.document.fold_starting_at(source_line) else {
            return false;
        };
        let mut folds = state.document.folds().to_vec();
        let Some(fold) = folds
            .iter_mut()
            .find(|fold| fold.start_line == target.start_line && fold.end_line == target.end_line)
        else {
            return false;
        };
        fold.expanded = !fold.expanded;
        let expanded = fold.expanded;
        if !fold.expanded {
            let (selection_line, _) = state.document.line_column_for_offset(state.selection.focus);
            if selection_line > fold.start_line && selection_line <= fold.end_line {
                let caret = state
                    .document
                    .lines()
                    .get(fold.start_line)
                    .map_or(0, |line| line.end);
                state.selection = EditorSelection {
                    anchor: caret,
                    focus: caret,
                };
            }
        }
        let text = state.document.text().to_string();
        let syntax = state.document.syntax().to_vec();
        state.document = EditorDocument::new_with_folds(text, syntax, folds);
        state.clamp_scroll(self.bounds().size);
        let selection = selection_utf16(&state.document, state.selection);
        drop(state);
        log::debug!(
            "native Skia editor fold toggled range={}..{} expanded={expanded}",
            target.start_line,
            target.end_line
        );
        self.notify_selection_changed(selection);
        self.notify_scroll_changed();
        self.update_accessibility();
        self.render_frame();
        true
    }

    fn fold_selection(&self) {
        let mut state = self.ivars().state.borrow_mut();
        let (start, end) = state.selection.normalized();
        if start >= end {
            return;
        }
        let (start_line, _) = state.document.line_column_for_offset(start);
        let (end_line, _) = state
            .document
            .line_column_for_offset(previous_char_boundary(state.document.text(), end));
        if end_line <= start_line {
            return;
        }
        if !state
            .manual_folds
            .iter()
            .any(|range| *range == (start_line, end_line))
        {
            state.manual_folds.push((start_line, end_line));
        }
        let mut folds = state.document.folds().to_vec();
        if let Some(fold) = folds
            .iter_mut()
            .find(|fold| fold.start_line == start_line && fold.end_line == end_line)
        {
            fold.expanded = false;
        } else {
            folds.push(EditorFoldRange {
                start_line,
                end_line,
                expanded: false,
            });
        }
        let caret = state
            .document
            .lines()
            .get(start_line)
            .map_or(0, |line| line.end);
        state.selection = EditorSelection {
            anchor: caret,
            focus: caret,
        };
        let text = state.document.text().to_string();
        let syntax = state.document.syntax().to_vec();
        state.document = EditorDocument::new_with_folds(text, syntax, folds);
        state.clamp_scroll(self.bounds().size);
        let selection = selection_utf16(&state.document, state.selection);
        drop(state);
        log::debug!("native Skia editor folded selection lines={start_line}..{end_line}");
        self.notify_selection_changed(selection);
        self.notify_scroll_changed();
        self.update_accessibility();
        self.render_frame();
    }

    fn unfold_all(&self) {
        let mut state = self.ivars().state.borrow_mut();
        if state.document.folds().iter().all(|fold| fold.expanded) {
            return;
        }
        let mut folds = state.document.folds().to_vec();
        for fold in &mut folds {
            fold.expanded = true;
        }
        let text = state.document.text().to_string();
        let syntax = state.document.syntax().to_vec();
        state.document = EditorDocument::new_with_folds(text, syntax, folds);
        state.clamp_scroll(self.bounds().size);
        drop(state);
        log::debug!("native Skia editor expanded all folds");
        self.notify_scroll_changed();
        self.update_accessibility();
        self.render_frame();
    }

    fn toggle_line_comment(&self) {
        let mut state = self.ivars().state.borrow_mut();
        if !state.editable {
            return;
        }
        let language = language_support_for_id(language_id_from_path(&state.path));
        let Some(prefix) = language.line_comment else {
            log::debug!(
                "native Skia editor line comment skipped unsupported language={:?}",
                language.id
            );
            return;
        };
        let Some(edit) = toggle_editor_line_comment(state.document.text(), state.selection, prefix)
        else {
            log::debug!("native Skia editor line comment skipped no applicable lines");
            return;
        };
        let snapshot = EditorSnapshot {
            text: state.document.text().to_string(),
            selection: state.selection,
        };
        state.undo.push(snapshot);
        if state.undo.len() > 100 {
            state.undo.remove(0);
        }
        state.redo.clear();
        let mut text = state.document.text().to_string();
        text.replace_range(edit.range_start..edit.range_end, &edit.replacement);
        state.document = EditorDocument::new(text, Vec::new());
        state.diagnostics.clear();
        state.manual_folds.clear();
        clear_completion_state(&mut state);
        state.selection = edit.map_selection(state.selection);
        rebuild_search_state(&mut state);
        state.reveal_selection(self.bounds().size);
        let text = state.document.text().to_string();
        let selection = selection_utf16(&state.document, state.selection);
        let action = if edit.uncomment {
            "uncomment"
        } else {
            "comment"
        };
        log::debug!(
            "native Skia editor line comment action={action} range={}..{} lines={}",
            edit.range_start,
            edit.range_end,
            edit.line_count()
        );
        drop(state);
        self.notify_text_changed(text, selection);
        self.notify_scroll_changed();
        self.update_accessibility();
        self.render_frame();
    }

    fn replace_text(&self, replacement: String, replacement_range: NSRange) {
        let mut state = self.ivars().state.borrow_mut();
        if !state.editable {
            return;
        }
        let (start, end) = if replacement_range.location == NSNotFound as usize {
            state.selection.normalized()
        } else {
            byte_range_for_utf16(state.document.text(), replacement_range)
        };
        let mut text = state.document.text().to_string();
        let snapshot = EditorSnapshot {
            text: text.clone(),
            selection: state.selection,
        };
        state.undo.push(snapshot);
        if state.undo.len() > 100 {
            state.undo.remove(0);
        }
        state.redo.clear();
        text.replace_range(start..end, &replacement);
        let caret = start + replacement.len();
        state.document = EditorDocument::new(text, Vec::new());
        state.diagnostics.clear();
        state.manual_folds.clear();
        clear_completion_state(&mut state);
        state.selection = EditorSelection {
            anchor: caret,
            focus: caret,
        };
        rebuild_search_state(&mut state);
        state.reveal_selection(self.bounds().size);
        let text = state.document.text().to_string();
        let selection = selection_utf16(&state.document, state.selection);
        drop(state);
        self.notify_text_changed(text, selection);
        self.notify_scroll_changed();
        self.update_accessibility();
        self.render_frame();
    }

    fn delete_backward(&self) {
        let mut state = self.ivars().state.borrow_mut();
        if state.selection.is_empty() {
            let caret = state.selection.focus;
            let previous = previous_char_boundary(state.document.text(), caret);
            state.selection = EditorSelection {
                anchor: previous,
                focus: caret,
            };
        }
        drop(state);
        self.replace_text(String::new(), NSRange::new(NSNotFound as usize, 0));
    }

    fn delete_forward(&self) {
        let mut state = self.ivars().state.borrow_mut();
        if state.selection.is_empty() {
            let caret = state.selection.focus;
            let next = next_char_boundary(state.document.text(), caret);
            state.selection = EditorSelection {
                anchor: caret,
                focus: next,
            };
        }
        drop(state);
        self.replace_text(String::new(), NSRange::new(NSNotFound as usize, 0));
    }

    fn delete_word(&self, forward: bool) {
        let mut state = self.ivars().state.borrow_mut();
        if state.selection.is_empty() {
            let caret = state.selection.focus;
            let boundary = if forward {
                next_word_boundary(state.document.text(), caret)
            } else {
                previous_word_boundary(state.document.text(), caret)
            };
            state.selection = EditorSelection {
                anchor: caret,
                focus: boundary,
            };
        }
        drop(state);
        self.replace_text(String::new(), NSRange::new(NSNotFound as usize, 0));
    }

    fn move_horizontal(&self, direction: isize, modify: bool) {
        let mut state = self.ivars().state.borrow_mut();
        clear_completion_state(&mut state);
        let (start, end) = state.selection.normalized();
        let focus = if !modify && !state.selection.is_empty() {
            if direction < 0 { start } else { end }
        } else if direction < 0 {
            previous_char_boundary(state.document.text(), state.selection.focus)
        } else {
            next_char_boundary(state.document.text(), state.selection.focus)
        };
        state.selection = EditorSelection {
            anchor: if modify {
                state.selection.anchor
            } else {
                focus
            },
            focus,
        };
        state.reveal_selection(self.bounds().size);
        let selection = selection_utf16(&state.document, state.selection);
        drop(state);
        self.notify_selection_changed(selection);
        self.notify_scroll_changed();
        self.update_accessibility();
        self.render_frame();
    }

    fn move_word(&self, forward: bool, modify: bool) {
        let mut state = self.ivars().state.borrow_mut();
        clear_completion_state(&mut state);
        let (start, end) = state.selection.normalized();
        let focus = if !modify && !state.selection.is_empty() {
            if forward { end } else { start }
        } else if forward {
            next_word_boundary(state.document.text(), state.selection.focus)
        } else {
            previous_word_boundary(state.document.text(), state.selection.focus)
        };
        state.selection = EditorSelection {
            anchor: if modify {
                state.selection.anchor
            } else {
                focus
            },
            focus,
        };
        state.reveal_selection(self.bounds().size);
        let selection = selection_utf16(&state.document, state.selection);
        drop(state);
        self.notify_selection_changed(selection);
        self.notify_scroll_changed();
        self.update_accessibility();
        self.render_frame();
    }

    fn move_vertical(&self, direction: isize, modify: bool) {
        let mut state = self.ivars().state.borrow_mut();
        clear_completion_state(&mut state);
        let (line, column) = state.document.line_column_for_offset(state.selection.focus);
        let target_line = if direction < 0 {
            line.saturating_sub(1)
        } else {
            (line + 1).min(state.document.lines().len().saturating_sub(1))
        };
        let focus = state.document.offset_for_line_column(target_line, column);
        state.selection = EditorSelection {
            anchor: if modify {
                state.selection.anchor
            } else {
                focus
            },
            focus,
        };
        state.reveal_selection(self.bounds().size);
        let selection = selection_utf16(&state.document, state.selection);
        drop(state);
        self.notify_selection_changed(selection);
        self.notify_scroll_changed();
        self.update_accessibility();
        self.render_frame();
    }

    fn move_to_line_edge(&self, end: bool, modify: bool) {
        let mut state = self.ivars().state.borrow_mut();
        clear_completion_state(&mut state);
        let (line, _) = state.document.line_column_for_offset(state.selection.focus);
        let focus = state
            .document
            .lines()
            .get(line)
            .map_or(0, |line| if end { line.end } else { line.start });
        state.selection = EditorSelection {
            anchor: if modify {
                state.selection.anchor
            } else {
                focus
            },
            focus,
        };
        state.reveal_selection(self.bounds().size);
        let selection = selection_utf16(&state.document, state.selection);
        drop(state);
        self.notify_selection_changed(selection);
        self.notify_scroll_changed();
        self.update_accessibility();
        self.render_frame();
    }

    fn move_to_document_edge(&self, end: bool, modify: bool) {
        let mut state = self.ivars().state.borrow_mut();
        clear_completion_state(&mut state);
        let focus = if end { state.document.text().len() } else { 0 };
        state.selection = EditorSelection {
            anchor: if modify {
                state.selection.anchor
            } else {
                focus
            },
            focus,
        };
        state.reveal_selection(self.bounds().size);
        let selection = selection_utf16(&state.document, state.selection);
        drop(state);
        self.notify_selection_changed(selection);
        self.notify_scroll_changed();
        self.update_accessibility();
        self.render_frame();
    }

    fn page(&self, direction: isize) {
        let mut state = self.ivars().state.borrow_mut();
        let delta = state.viewport.height() * direction as f64;
        let CodeState {
            document,
            viewport,
            metrics,
            ..
        } = &mut *state;
        viewport.scroll_by(0.0, delta, document, *metrics);
        drop(state);
        self.notify_scroll_changed();
        self.update_accessibility_viewport();
        self.render_frame();
    }

    fn copy_selection(&self) {
        let state = self.ivars().state.borrow();
        let text = selected_editor_text(&state.document, state.selection);
        if text.is_empty() {
            return;
        }
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        pasteboard.setString_forType(&NSString::from_str(&text), unsafe {
            NSPasteboardTypeString
        });
    }

    fn cut_selection(&self) {
        if !self.ivars().state.borrow().editable {
            return;
        }
        self.copy_selection();
        self.replace_text(String::new(), NSRange::new(NSNotFound as usize, 0));
    }

    fn paste_clipboard(&self) {
        if !self.ivars().state.borrow().editable {
            return;
        }
        let pasteboard = NSPasteboard::generalPasteboard();
        if let Some(text) = pasteboard.stringForType(unsafe { NSPasteboardTypeString }) {
            self.replace_text(text.to_string(), NSRange::new(NSNotFound as usize, 0));
        }
    }

    fn select_all_internal(&self) {
        let mut state = self.ivars().state.borrow_mut();
        state.selection = EditorSelection {
            anchor: 0,
            focus: state.document.text().len(),
        };
        let selection = selection_utf16(&state.document, state.selection);
        drop(state);
        self.notify_selection_changed(selection);
        self.update_accessibility();
        self.render_frame();
    }

    fn undo(&self) {
        let mut state = self.ivars().state.borrow_mut();
        let Some(snapshot) = state.undo.pop() else {
            return;
        };
        let current = EditorSnapshot {
            text: state.document.text().to_string(),
            selection: state.selection,
        };
        state.redo.push(current);
        state.document = EditorDocument::new(snapshot.text, Vec::new());
        state.diagnostics.clear();
        state.manual_folds.clear();
        clear_completion_state(&mut state);
        state.selection = snapshot.selection;
        rebuild_search_state(&mut state);
        state.reveal_selection(self.bounds().size);
        let text = state.document.text().to_string();
        let selection = selection_utf16(&state.document, state.selection);
        drop(state);
        self.notify_text_changed(text, selection);
        self.notify_scroll_changed();
        self.update_accessibility();
        self.render_frame();
    }

    fn redo(&self) {
        let mut state = self.ivars().state.borrow_mut();
        let Some(snapshot) = state.redo.pop() else {
            return;
        };
        let current = EditorSnapshot {
            text: state.document.text().to_string(),
            selection: state.selection,
        };
        state.undo.push(current);
        state.document = EditorDocument::new(snapshot.text, Vec::new());
        state.diagnostics.clear();
        state.manual_folds.clear();
        clear_completion_state(&mut state);
        state.selection = snapshot.selection;
        rebuild_search_state(&mut state);
        state.reveal_selection(self.bounds().size);
        let text = state.document.text().to_string();
        let selection = selection_utf16(&state.document, state.selection);
        drop(state);
        self.notify_text_changed(text, selection);
        self.notify_scroll_changed();
        self.update_accessibility();
        self.render_frame();
    }

    fn update_accessibility(&self) {
        let state = self.ivars().state.borrow();
        self.setAccessibilityLabel(Some(&NSString::from_str(if state.path.is_empty() {
            "Code editor"
        } else {
            &state.path
        })));
        unsafe {
            self.setAccessibilityValue(Some(&NSString::from_str(state.document.text())));
        }
        self.setAccessibilitySelectedText(Some(&NSString::from_str(&selected_editor_text(
            &state.document,
            state.selection,
        ))));
        self.setAccessibilitySelectedTextRange(selection_utf16(&state.document, state.selection));
        self.setAccessibilityNumberOfCharacters(state.document.text().encode_utf16().count() as _);
        let (line, _) = state.document.line_column_for_offset(state.selection.focus);
        self.setAccessibilityInsertionPointLineNumber(line as _);
        let visible = state
            .viewport
            .visible_byte_range(&state.document, state.metrics);
        self.setAccessibilityVisibleCharacterRange(utf16_range_for_bytes(
            state.document.text(),
            visible.start,
            visible.end,
        ));
    }

    fn update_accessibility_viewport(&self) {
        let state = self.ivars().state.borrow();
        let visible = state
            .viewport
            .visible_byte_range(&state.document, state.metrics);
        self.setAccessibilityVisibleCharacterRange(utf16_range_for_bytes(
            state.document.text(),
            visible.start,
            visible.end,
        ));
    }

    fn notify_scroll_changed(&self) {
        if let Some(delegate) = self.ivars().delegate.borrow().load() {
            delegate.files_code_scroll_changed();
        }
    }

    fn notify_text_changed(&self, text: String, selection: NSRange) {
        if let Some(delegate) = self.ivars().delegate.borrow().load() {
            delegate.files_code_text_changed(text, selection);
        }
    }

    fn notify_selection_changed(&self, selection: NSRange) {
        if let Some(delegate) = self.ivars().delegate.borrow().load() {
            delegate.files_code_selection_changed(selection);
        }
    }
}

fn editor_line_bounds(document: &EditorDocument, offset: usize) -> Option<(usize, usize)> {
    let (line, _) = document.line_column_for_offset(offset);
    document
        .lines()
        .get(line)
        .map(|line| (line.start, line.end_with_newline))
}

fn rebuild_search_state(state: &mut CodeState) {
    state.search_matches = find_editor_search_matches(state.document.text(), &state.search_query);
    state.active_search_match =
        editor_search_index_after(&state.search_matches, state.selection.focus);
}

fn clear_completion_state(state: &mut CodeState) {
    state.completion_items.clear();
    state.completion_selected = 0;
    state.completion_range = None;
}

fn activate_search_match(state: &mut CodeState, viewport: NSSize) {
    let Some(search_match) = state
        .active_search_match
        .and_then(|index| state.search_matches.get(index))
        .copied()
    else {
        return;
    };
    state.selection = EditorSelection {
        anchor: search_match.start,
        focus: search_match.end,
    };
    state.reveal_selection(viewport);
}

fn utf16_offset_for_byte(text: &str, byte: usize) -> usize {
    text.get(..byte.min(text.len()))
        .unwrap_or_default()
        .encode_utf16()
        .count()
}

fn byte_offset_for_utf16(text: &str, target: usize) -> usize {
    let mut units = 0;
    for (byte, character) in text.char_indices() {
        if units >= target {
            return byte;
        }
        units += character.len_utf16();
        if units > target {
            return byte;
        }
    }
    text.len()
}

fn byte_range_for_utf16(text: &str, range: NSRange) -> (usize, usize) {
    let start = byte_offset_for_utf16(text, range.location);
    let end = byte_offset_for_utf16(text, range.location.saturating_add(range.length));
    (start.min(end), start.max(end))
}

fn utf16_range_for_bytes(text: &str, start: usize, end: usize) -> NSRange {
    let start_utf16 = utf16_offset_for_byte(text, start);
    let end_utf16 = utf16_offset_for_byte(text, end);
    NSRange::new(start_utf16, end_utf16.saturating_sub(start_utf16))
}

fn selection_utf16(document: &EditorDocument, selection: EditorSelection) -> NSRange {
    let (start, end) = selection.normalized();
    utf16_range_for_bytes(document.text(), start, end)
}

fn previous_char_boundary(text: &str, offset: usize) -> usize {
    text.get(..offset.min(text.len()))
        .and_then(|prefix| prefix.char_indices().next_back().map(|(byte, _)| byte))
        .unwrap_or(0)
}

fn next_char_boundary(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    text.get(offset..)
        .and_then(|suffix| {
            suffix
                .chars()
                .next()
                .map(|character| offset + character.len_utf8())
        })
        .unwrap_or(text.len())
}

fn editor_input_text(value: &AnyObject) -> String {
    if let Some(text) = value.downcast_ref::<NSString>() {
        text.to_string()
    } else if let Some(text) = value.downcast_ref::<NSAttributedString>() {
        text.string().to_string()
    } else {
        String::new()
    }
}
