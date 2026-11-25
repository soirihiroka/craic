use crate::application::AppDelegate;
use craic_language::language_id_from_path;
use craic_render_skia::{
    DragSelection, EditorDocument, EditorFoldRange, EditorMetrics, EditorSelection,
    TextDiagnosticSpan, TextSyntaxSpan, VERTICAL_SCROLLBAR_WIDTH, previous_char_boundary,
    selected_editor_text, vertical_scrollbar_layout, vertical_scrollbar_scroll_for_drag,
    vertical_scrollbar_scroll_for_press,
};
use craic_ui_code::{
    EDITOR_FONT_FAMILY, Editor as CodeState, EditorFrame, EditorScene, Motion, SearchOptions,
    load_editor_font, paint_editor,
};
use gpui::Rgba;
use gpui_apple::metal_renderer::{Context as MetalContext, MetalRenderer};
use gpui_wgpu::CosmicTextSystem;
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, Sel};
use objc2::{AnyThread, DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityTextAreaRole, NSAutoresizingMaskOptions, NSCursor, NSEvent,
    NSEventModifierFlags, NSMenu, NSMenuItem, NSPasteboard, NSPasteboardTypeString,
    NSTextInputClient, NSTrackingArea, NSTrackingAreaOptions, NSView,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSAttributedString, NSAttributedStringKey, NSNotFound,
    NSObjectProtocol, NSRange, NSRangePointer, NSRect, NSSize, NSString, NSUInteger,
};
use std::cell::{Cell, RefCell};

const SCROLLBAR_WIDTH: f64 = VERTICAL_SCROLLBAR_WIDTH;

thread_local! {
    static EDITOR_TEXT_SYSTEM: CosmicTextSystem = {
        let text_system = CosmicTextSystem::new(EDITOR_FONT_FAMILY);
        if let Err(error) = load_editor_font(&text_system) {
            log::error!("failed to load the GPUI code editor font: {error}");
        }
        text_system
    };
    static EDITOR_METAL_CONTEXT: MetalContext = MetalContext::default();
}

struct CodeRenderer {
    metal: MetalRenderer,
    scene: EditorScene,
    logged_frame_error: bool,
}

pub(crate) struct CodeMetalViewIvars {
    renderer: RefCell<Option<CodeRenderer>>,
    renderer_requested: Cell<bool>,
    state: RefCell<CodeState>,
    delegate: RefCell<Weak<AppDelegate>>,
    dragging_selection: Cell<Option<DragSelection<usize>>>,
    tracking_area: RefCell<Option<Retained<NSTrackingArea>>>,
    scrollbar_hovered: Cell<bool>,
    scrollbar_active: Cell<bool>,
    scrollbar_drag_start_y: Cell<f64>,
    scrollbar_drag_start_scroll: Cell<f64>,
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
                self.delete(false, false);
            } else if selector == sel!(deleteForward:) {
                self.delete(true, false);
            } else if selector == sel!(deleteWordBackward:) {
                self.delete(false, true);
            } else if selector == sel!(deleteWordForward:) {
                self.delete(true, true);
            } else if selector == sel!(moveLeft:) {
                self.move_selection(Motion::Character, false, false);
            } else if selector == sel!(moveRight:) {
                self.move_selection(Motion::Character, true, false);
            } else if selector == sel!(moveLeftAndModifySelection:) {
                self.move_selection(Motion::Character, false, true);
            } else if selector == sel!(moveRightAndModifySelection:) {
                self.move_selection(Motion::Character, true, true);
            } else if selector == sel!(moveWordLeft:) {
                self.move_selection(Motion::Word, false, false);
            } else if selector == sel!(moveWordRight:) {
                self.move_selection(Motion::Word, true, false);
            } else if selector == sel!(moveWordLeftAndModifySelection:) {
                self.move_selection(Motion::Word, false, true);
            } else if selector == sel!(moveWordRightAndModifySelection:) {
                self.move_selection(Motion::Word, true, true);
            } else if selector == sel!(moveUp:) {
                if !self.select_completion(-1) {
                    self.move_selection(Motion::Row, false, false);
                }
            } else if selector == sel!(moveDown:) {
                if !self.select_completion(1) {
                    self.move_selection(Motion::Row, true, false);
                }
            } else if selector == sel!(moveUpAndModifySelection:) {
                self.move_selection(Motion::Row, false, true);
            } else if selector == sel!(moveDownAndModifySelection:) {
                self.move_selection(Motion::Row, true, true);
            } else if selector == sel!(moveToBeginningOfLine:)
                || selector == sel!(moveToLeftEndOfLine:)
            {
                self.move_selection(Motion::LineEdge, false, false);
            } else if selector == sel!(moveToEndOfLine:)
                || selector == sel!(moveToRightEndOfLine:)
            {
                self.move_selection(Motion::LineEdge, true, false);
            } else if selector == sel!(moveToBeginningOfLineAndModifySelection:)
                || selector == sel!(moveToLeftEndOfLineAndModifySelection:)
            {
                self.move_selection(Motion::LineEdge, false, true);
            } else if selector == sel!(moveToEndOfLineAndModifySelection:)
                || selector == sel!(moveToRightEndOfLineAndModifySelection:)
            {
                self.move_selection(Motion::LineEdge, true, true);
            } else if selector == sel!(moveToBeginningOfDocument:) {
                self.move_selection(Motion::DocumentEdge, false, false);
            } else if selector == sel!(moveToEndOfDocument:) {
                self.move_selection(Motion::DocumentEdge, true, false);
            } else if selector == sel!(moveToBeginningOfDocumentAndModifySelection:) {
                self.move_selection(Motion::DocumentEdge, false, true);
            } else if selector == sel!(moveToEndOfDocumentAndModifySelection:) {
                self.move_selection(Motion::DocumentEdge, true, true);
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
            let (start, end) = crate::text_offsets::byte_range(state.document.text(), range);
            if !actual_range.is_null() {
                unsafe {
                    *actual_range = crate::text_offsets::range_for_bytes(state.document.text(), start, end)
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
            let byte = crate::text_offsets::byte_offset_clamped(state.document.text(), range.location);
            let line = state.document.visual_line_for_offset(byte);
            let column = state.document.visual_column_for_offset(byte);
            let local = NSRect::new(
                objc2_foundation::NSPoint::new(
                    state.metrics.gutter_width as f64
                        + state.metrics.text_inset as f64
                        + column * state.metrics.char_width as f64
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
            crate::text_offsets::offset_for_byte(state.document.text(), byte)
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

        #[unsafe(method(viewDidMoveToWindow))]
        fn view_did_move_to_window(&self) {
            if let Some(window) = self.window() {
                window.setAcceptsMouseMovedEvents(true);
                self.render_frame();
            }
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: NSSize) {
            unsafe { let _: () = msg_send![super(self), setFrameSize: size]; }
            self.ivars().state.borrow_mut().resize(size.width, size.height);
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

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            self.set_scrollbar_hovered(self.point_in_scrollbar_lane(point));
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.set_scrollbar_hovered(false);
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            if let Some(window) = self.window() { window.makeFirstResponder(Some(self)); }
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            let bounds = self.bounds();
            if self.point_in_scrollbar_lane(point) {
                self.set_scrollbar_hovered(true);
                let mut state = self.ivars().state.borrow_mut();
                let (_, total_height) = state.document.content_size(state.metrics);
                if let Some(layout) = vertical_scrollbar_layout(
                    bounds.size.width,
                    bounds.size.height,
                    total_height as f64,
                    state.viewport.scroll_y(),
                    1.0,
                ) {
                    let scroll_y = vertical_scrollbar_scroll_for_press(layout, point.y);
                    let current_scroll = state.viewport.scroll_y();
                    let CodeState { document, viewport, metrics, .. } = &mut *state;
                    viewport.scroll_by(0.0, scroll_y - current_scroll, document, *metrics);
                    self.ivars().scrollbar_drag_start_y.set(point.y);
                    self.ivars().scrollbar_drag_start_scroll.set(scroll_y);
                    self.ivars().scrollbar_active.set(true);
                    self.ivars().dragging_selection.set(None);
                    drop(state);
                    self.notify_scroll_changed();
                    self.update_accessibility_viewport();
                    self.render_frame();
                    return;
                }
            }
            if point.x <= 28.0 && self.toggle_fold_at_y(point.y) {
                return;
            }
            let extend = event.modifierFlags().contains(NSEventModifierFlags::Shift);
            let drag = self.ivars().state.borrow_mut().start_drag(
                point.x, point.y, event.clickCount() as i32, extend,
            );
            self.ivars().dragging_selection.set(Some(drag));
            self.update_accessibility();
            self.render_frame();
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            if self.ivars().scrollbar_active.get() {
                let point = self.convertPoint_fromView(event.locationInWindow(), None);
                let bounds = self.bounds();
                let mut state = self.ivars().state.borrow_mut();
                let (_, total_height) = state.document.content_size(state.metrics);
                if let Some(layout) = vertical_scrollbar_layout(
                    bounds.size.width,
                    bounds.size.height,
                    total_height as f64,
                    state.viewport.scroll_y(),
                    1.0,
                ) {
                    let scroll_y = vertical_scrollbar_scroll_for_drag(
                        layout,
                        self.ivars().scrollbar_drag_start_scroll.get(),
                        point.y - self.ivars().scrollbar_drag_start_y.get(),
                    );
                    let current_scroll = state.viewport.scroll_y();
                    let CodeState { document, viewport, metrics, .. } = &mut *state;
                    viewport.scroll_by(0.0, scroll_y - current_scroll, document, *metrics);
                }
                drop(state);
                self.notify_scroll_changed();
                self.update_accessibility_viewport();
                self.render_frame();
                return;
            }
            let Some(drag) = self.ivars().dragging_selection.get() else {
                return;
            };
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            let scrolled = self.ivars().state.borrow_mut().drag_to(drag, point.x, point.y);
            if scrolled {
                self.notify_scroll_changed();
            }
            self.update_accessibility();
            self.render_frame();
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            self.ivars().dragging_selection.set(None);
            if self.ivars().scrollbar_active.replace(false) {
                let point = self.convertPoint_fromView(event.locationInWindow(), None);
                self.set_scrollbar_hovered(self.point_in_scrollbar_lane(point));
                self.render_frame();
            }
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
            renderer: RefCell::new(None),
            renderer_requested: Cell::new(false),
            state: RefCell::new(CodeState::new(font_size as f32)),
            delegate: RefCell::new(Weak::default()),
            dragging_selection: Cell::new(None),
            tracking_area: RefCell::new(None),
            scrollbar_hovered: Cell::new(false),
            scrollbar_active: Cell::new(false),
            scrollbar_drag_start_y: Cell::new(0.0),
            scrollbar_drag_start_scroll: Cell::new(0.0),
        });
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        view.initialize_tracking();
        view.setAccessibilityElement(true);
        view.setAccessibilityRole(Some(unsafe { NSAccessibilityTextAreaRole }));
        view.setAccessibilityLabel(Some(&NSString::from_str("Code editor")));
        view.update_accessibility();
        view
    }

    pub fn attach_delegate(&self, delegate: &AppDelegate) {
        self.ivars().delegate.replace(Weak::new(delegate));
        log::debug!("native code editor attached its weak lifecycle delegate");
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
        state.language = language_id_from_path(path);
        state.editable = editable;
        let bounds = self.bounds();
        state.resize(bounds.size.width, bounds.size.height);
        state.reset_document(text, syntax, selection, reset_scroll);
        drop(state);
        self.update_accessibility();
        self.render_frame();
    }

    pub fn set_syntax_and_folds(&self, syntax: Vec<TextSyntaxSpan>, ranges: Vec<(usize, usize)>) {
        self.ivars()
            .state
            .borrow_mut()
            .set_syntax_and_folds(syntax, ranges);
        self.update_accessibility_viewport();
        self.render_frame();
    }

    pub fn set_diagnostics(&self, diagnostics: Vec<TextDiagnosticSpan>) {
        self.ivars().state.borrow_mut().set_diagnostics(diagnostics);
        self.render_frame();
    }

    pub fn set_completions(&self, items: Vec<String>, replacement_range: NSRange) {
        let mut state = self.ivars().state.borrow_mut();
        let range = crate::text_offsets::byte_range(state.document.text(), replacement_range);
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
        state.clamp_scroll();
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
        let _ = state.set_search(query, SearchOptions::default());
        drop(state);
        self.selection_changed();
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
        self.ivars().state.borrow_mut().clear_search();
        self.render_frame();
    }

    fn step_search(&self, direction: isize) {
        self.ivars().state.borrow_mut().step_search(direction > 0);
        self.selection_changed();
    }

    fn select_completion(&self, direction: isize) -> bool {
        let changed = self
            .ivars()
            .state
            .borrow_mut()
            .step_completion(direction > 0);
        if changed {
            self.render_frame();
        }
        changed
    }

    fn accept_completion(&self) -> bool {
        let changed = self.ivars().state.borrow_mut().accept_completion();
        if changed {
            self.text_changed();
        }
        changed
    }

    fn dismiss_completion(&self) {
        self.ivars().state.borrow_mut().clear_completion();
        self.render_frame();
    }

    pub fn preview_source_offset(&self) -> usize {
        let state = self.ivars().state.borrow();
        state.viewport.source_offset(&state.document, state.metrics)
    }

    pub fn teardown_renderer(&self) {
        self.ivars().renderer_requested.set(true);
        if self.ivars().renderer.borrow_mut().take().is_some() {
            self.setLayer(None);
            self.setWantsLayer(false);
            log::debug!("released the native GPUI code editor surface");
        }
    }

    fn render_frame(&self) {
        let Some(window) = self.window() else {
            return;
        };
        if self.isHiddenOrHasHiddenAncestor() {
            return;
        }
        let bounds = self.bounds();
        if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
            return;
        }
        let Ok(mut renderer) = self.ivars().renderer.try_borrow_mut() else {
            return;
        };
        if !self.ivars().renderer_requested.replace(true) {
            let metal = match EDITOR_METAL_CONTEXT
                .with(|context| MetalRenderer::new(context.clone(), false, false))
            {
                Ok(metal) => metal,
                Err(error) => {
                    log::error!(
                        "failed to initialize the native GPUI code editor surface: {error:#}"
                    );
                    return;
                }
            };
            if let Some(layer) = metal.layer() {
                layer.setAllowsNextDrawableTimeout(true);
                self.setWantsLayer(true);
                self.setLayer(Some(layer));
            }
            *renderer = Some(CodeRenderer {
                metal,
                scene: EditorScene::default(),
                logged_frame_error: false,
            });
            log::debug!("initialized the native GPUI code editor surface");
        }
        let Some(renderer) = renderer.as_mut() else {
            return;
        };
        let mut frame = EditorFrame::new(
            bounds.size.width as f32,
            bounds.size.height as f32,
            window.backingScaleFactor() as f32,
        );
        let state = self.ivars().state.borrow();
        paint_editor(&mut frame, &state, self.is_focused(), true);
        self.draw_scrollbar(&mut frame, &state, bounds.size);
        drop(state);
        let result = EDITOR_TEXT_SYSTEM.with(|text_system| {
            frame.build_scene(
                &mut renderer.scene,
                text_system,
                renderer.metal.sprite_atlas().as_ref(),
            )
        });
        if let Err(error) = result {
            if !renderer.logged_frame_error {
                renderer.logged_frame_error = true;
                log::error!("failed to draw the native GPUI code editor: {error}");
            }
            return;
        }
        if let Some(layer) = renderer.metal.layer() {
            layer.setContentsScale(frame.scale as f64);
        }
        renderer.metal.update_drawable_size(frame.device_size());
        renderer.metal.draw(&renderer.scene.scene);
    }

    fn draw_scrollbar(&self, frame: &mut EditorFrame, state: &CodeState, viewport: NSSize) {
        let (_, height) = state.document.content_size(state.metrics);
        let hover = f64::from(self.ivars().scrollbar_hovered.get());
        let Some(layout) = vertical_scrollbar_layout(
            viewport.width,
            viewport.height,
            height as f64,
            state.viewport.scroll_y(),
            hover,
        ) else {
            return;
        };
        let mut rounded = |rect: craic_render_skia::Rect, color: Rgba| {
            frame.quad(
                rect.x as f32,
                rect.y as f32,
                rect.width as f32,
                rect.height as f32,
                (rect.width / 2.0) as f32,
                color,
            );
        };
        if hover > f64::EPSILON {
            rounded(
                layout.handle,
                Rgba {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 0.10,
                },
            );
        }
        let active = self.ivars().scrollbar_active.get();
        rounded(
            craic_render_skia::Rect {
                x: layout.thumb.x - 1.0,
                y: layout.thumb.y - 1.0,
                width: layout.thumb.width + 2.0,
                height: layout.thumb.height + 2.0,
            },
            Rgba {
                a: if active {
                    0.57
                } else {
                    0.3325 + 0.2375 * hover as f32
                },
                ..Rgba::default()
            },
        );
        rounded(
            layout.thumb,
            Rgba {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: if active {
                    0.60
                } else {
                    0.20 + 0.20 * hover as f32
                },
            },
        );
    }

    fn initialize_tracking(&self) {
        let tracking = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                NSRect::ZERO,
                NSTrackingAreaOptions::MouseEnteredAndExited
                    | NSTrackingAreaOptions::MouseMoved
                    | NSTrackingAreaOptions::ActiveInActiveApp
                    | NSTrackingAreaOptions::InVisibleRect
                    | NSTrackingAreaOptions::EnabledDuringMouseDrag,
                Some(self),
                None,
            )
        };
        self.addTrackingArea(&tracking);
        self.ivars().tracking_area.replace(Some(tracking));
    }

    fn point_in_scrollbar_lane(&self, point: objc2_foundation::NSPoint) -> bool {
        let bounds = self.bounds();
        let state = self.ivars().state.borrow();
        let (_, total_height) = state.document.content_size(state.metrics);
        point.x >= bounds.size.width - SCROLLBAR_WIDTH
            && point.x <= bounds.size.width
            && point.y >= 0.0
            && point.y <= bounds.size.height
            && vertical_scrollbar_layout(
                bounds.size.width,
                bounds.size.height,
                total_height as f64,
                state.viewport.scroll_y(),
                0.0,
            )
            .is_some()
    }

    fn set_scrollbar_hovered(&self, hovered: bool) {
        if self.ivars().scrollbar_hovered.replace(hovered) != hovered {
            self.render_frame();
        }
    }

    fn toggle_fold_at_y(&self, y: f64) -> bool {
        let changed = self.ivars().state.borrow_mut().toggle_fold(y);
        if changed {
            self.selection_changed();
        }
        changed
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
        state.clamp_scroll();
        let selection = selection_utf16(&state.document, state.selection);
        drop(state);
        log::debug!("native GPUI editor folded selection lines={start_line}..{end_line}");
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
        state.clamp_scroll();
        drop(state);
        log::debug!("native GPUI editor expanded all folds");
        self.notify_scroll_changed();
        self.update_accessibility();
        self.render_frame();
    }

    fn toggle_line_comment(&self) {
        let changed = self.ivars().state.borrow_mut().toggle_line_comment();
        if changed {
            self.text_changed();
        }
    }

    fn replace_text(&self, replacement: String, replacement_range: NSRange) {
        let mut state = self.ivars().state.borrow_mut();
        let range = (replacement_range.location != NSNotFound as usize).then(|| {
            let (start, end) =
                crate::text_offsets::byte_range(state.document.text(), replacement_range);
            start..end
        });
        let changed = state.replace_text(range, &replacement);
        drop(state);
        if changed {
            self.text_changed();
        }
    }

    fn delete(&self, forward: bool, word: bool) {
        let changed = self.ivars().state.borrow_mut().delete(forward, word);
        if changed {
            self.text_changed();
        }
    }

    fn move_selection(&self, motion: Motion, forward: bool, extend: bool) {
        self.ivars()
            .state
            .borrow_mut()
            .move_selection(motion, forward, extend);
        self.selection_changed();
    }

    fn selection_changed(&self) {
        let state = self.ivars().state.borrow();
        let selection = selection_utf16(&state.document, state.selection);
        drop(state);
        self.notify_selection_changed(selection);
        self.notify_scroll_changed();
        self.update_accessibility();
        self.render_frame();
    }

    fn text_changed(&self) {
        let state = self.ivars().state.borrow();
        let selection = selection_utf16(&state.document, state.selection);
        let text = state.document.text().to_owned();
        drop(state);
        self.notify_text_changed(text, selection);
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
        let changed = self.ivars().state.borrow_mut().undo(false);
        if changed {
            self.text_changed();
        }
    }

    fn redo(&self) {
        let changed = self.ivars().state.borrow_mut().undo(true);
        if changed {
            self.text_changed();
        }
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
        self.setAccessibilityVisibleCharacterRange(crate::text_offsets::range_for_bytes(
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
        self.setAccessibilityVisibleCharacterRange(crate::text_offsets::range_for_bytes(
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

fn selection_utf16(document: &EditorDocument, selection: EditorSelection) -> NSRange {
    let (start, end) = selection.normalized();
    crate::text_offsets::range_for_bytes(document.text(), start, end)
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
