use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::application::AppDelegate;
use craic_render_skia::{
    TerminalClipboard, TerminalMouseAction, TerminalMouseButton, TerminalMouseModifiers,
    TerminalPaintCache, TerminalPaintRequest, TerminalScroll, TerminalSearchDirection,
    TerminalSelectionType, TerminalSession, TerminalSide, TerminalSnapshot, TerminalSpawnOptions,
    TerminalViewport, paint_terminal,
};
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{
    AnyThread, ClassType, DefinedClass, MainThreadOnly, Message, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityTextAreaRole, NSButton, NSDragOperation, NSDraggingInfo,
    NSEvent, NSEventModifierFlags, NSEventTrackingRunLoopMode, NSMenu, NSMenuItem, NSPasteboard,
    NSPasteboardTypeFileURL, NSPasteboardTypeString, NSTextInputClient, NSView,
    NSWindowOcclusionState,
};
use objc2_core_foundation::CGSize;
use objc2_foundation::{
    MainThreadMarker, NSArray, NSAttributedString, NSAttributedStringKey, NSNotFound,
    NSObjectProtocol, NSPoint, NSRange, NSRangePointer, NSRect, NSRunLoop, NSRunLoopCommonModes,
    NSSize, NSString, NSUInteger, NSURL,
};
use objc2_metal::{
    MTLCommandBuffer, MTLCommandQueue, MTLCreateSystemDefaultDevice, MTLDevice, MTLDrawable,
    MTLPixelFormat,
};
use objc2_quartz_core::{
    CAMetalDisplayLink, CAMetalDisplayLinkDelegate, CAMetalDisplayLinkUpdate, CAMetalDrawable,
    CAMetalLayer,
};
use skia_safe::ColorType;
use skia_safe::gpu::{self, DirectContext, SurfaceOrigin, backend_render_targets, mtl};

const BASE_FONT_SIZE: f64 = 13.0;
const BASE_CELL_WIDTH: f64 = 8.0;
const BASE_CELL_HEIGHT: f64 = 18.0;
const REPORTED_ACTIVITY_TITLE_PREFIX: &str = "craic-terminal-activity:";

struct MetalState {
    _device: Retained<ProtocolObject<dyn MTLDevice>>,
    layer: Retained<CAMetalLayer>,
    command_queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    skia: DirectContext,
}

impl Drop for MetalState {
    fn drop(&mut self) {
        log::info!("releasing native Skia Metal terminal resources");
        self.skia.release_resources_and_abandon();
    }
}

pub(crate) struct TerminalMetalViewIvars {
    metal: RefCell<Option<MetalState>>,
    session: RefCell<Option<TerminalSession>>,
    snapshot: RefCell<TerminalSnapshot>,
    display_link: RefCell<Option<Retained<CAMetalDisplayLink>>>,
    window_occluded: Cell<bool>,
    dragging_selection: Cell<bool>,
    reported_mouse_button: Cell<Option<TerminalMouseButton>>,
    focused: Cell<bool>,
    cursor_visible: Cell<bool>,
    blink_timestamp: Cell<f64>,
    frame_dirty: Cell<bool>,
    paint_cache: RefCell<TerminalPaintCache>,
    accessibility_timestamp: Cell<Option<Instant>>,
    accessibility_dirty: Cell<bool>,
    marked_text: RefCell<String>,
    title: RefCell<String>,
    title_button: RefCell<Option<Retained<NSButton>>>,
    activation_delegate: RefCell<Weak<AppDelegate>>,
    session_id: Cell<isize>,
    exited: Cell<bool>,
    exit_code: Cell<Option<i32>>,
    font_size: Cell<f64>,
    cell_width: Cell<f64>,
    cell_height: Cell<f64>,
}

define_class!(
    // SAFETY: AppKit drawing, input, and Metal presentation remain on the main thread.
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = TerminalMetalViewIvars]
    pub(crate) struct TerminalMetalView;

    unsafe impl NSObjectProtocol for TerminalMetalView {}
    unsafe impl CAMetalDisplayLinkDelegate for TerminalMetalView {
        #[unsafe(method(metalDisplayLink:needsUpdate:))]
        fn metal_display_link_needs_update(
            &self,
            display_link: &CAMetalDisplayLink,
            update: &CAMetalDisplayLinkUpdate,
        ) {
            if !self.can_render() {
                display_link.setPaused(true);
                return;
            }
            self.drain_terminal_events();
            self.refresh_accessibility(false);
            let timestamp = update.targetTimestamp();
            if self.ivars().focused.get()
                && timestamp - self.ivars().blink_timestamp.get() >= 0.55
            {
                self.ivars().blink_timestamp.set(timestamp);
                self.ivars()
                    .cursor_visible
                    .set(!self.ivars().cursor_visible.get());
                self.ivars().frame_dirty.set(true);
            }
            if self.ivars().frame_dirty.replace(false) {
                self.render_drawable(&update.drawable(), false);
            }
            if self.ivars().exited.get() {
                display_link.setPaused(true);
            }
        }
    }
    unsafe impl NSTextInputClient for TerminalMetalView {
        #[unsafe(method(insertText:replacementRange:))]
        unsafe fn insert_text_replacement_range(
            &self,
            string: &AnyObject,
            _replacement_range: NSRange,
        ) {
            let text = terminal_input_text(string);
            self.ivars().marked_text.borrow_mut().clear();
            self.send_input(text.into_bytes());
        }

        #[unsafe(method(doCommandBySelector:))]
        unsafe fn do_command_by_selector(&self, selector: Sel) {
            let input = if selector == sel!(insertNewline:) {
                Some("\r")
            } else if selector == sel!(insertTab:) {
                Some("\t")
            } else if selector == sel!(deleteBackward:) {
                Some("\x7f")
            } else if selector == sel!(deleteForward:) {
                Some("\x1b[3~")
            } else if selector == sel!(cancelOperation:) {
                Some("\x1b")
            } else if selector == sel!(moveUp:) {
                Some("\x1b[A")
            } else if selector == sel!(moveDown:) {
                Some("\x1b[B")
            } else if selector == sel!(moveRight:) {
                Some("\x1b[C")
            } else if selector == sel!(moveLeft:) {
                Some("\x1b[D")
            } else if selector == sel!(moveToBeginningOfLine:) {
                Some("\x1b[H")
            } else if selector == sel!(moveToEndOfLine:) {
                Some("\x1b[F")
            } else if selector == sel!(pageUp:) {
                Some("\x1b[5~")
            } else if selector == sel!(pageDown:) {
                Some("\x1b[6~")
            } else {
                None
            };
            if let Some(input) = input {
                self.send_input(input.as_bytes().to_vec());
            }
        }

        #[unsafe(method(setMarkedText:selectedRange:replacementRange:))]
        unsafe fn set_marked_text_selected_range_replacement_range(
            &self,
            string: &AnyObject,
            _selected_range: NSRange,
            _replacement_range: NSRange,
        ) {
            self.ivars().marked_text.replace(terminal_input_text(string));
            self.render_frame();
        }

        #[unsafe(method(unmarkText))]
        fn unmark_text(&self) {
            let text = self.ivars().marked_text.take();
            if !text.is_empty() {
                self.send_input(text.into_bytes());
            }
        }

        #[unsafe(method(selectedRange))]
        fn selected_range(&self) -> NSRange {
            NSRange::new(NSNotFound as usize, 0)
        }

        #[unsafe(method(markedRange))]
        fn marked_range(&self) -> NSRange {
            let length = self.ivars().marked_text.borrow().encode_utf16().count();
            if length == 0 {
                NSRange::new(NSNotFound as usize, 0)
            } else {
                NSRange::new(0, length)
            }
        }

        #[unsafe(method(hasMarkedText))]
        fn has_marked_text(&self) -> bool {
            !self.ivars().marked_text.borrow().is_empty()
        }

        #[unsafe(method_id(attributedSubstringForProposedRange:actualRange:))]
        unsafe fn attributed_substring_for_proposed_range_actual_range(
            &self,
            _range: NSRange,
            actual_range: NSRangePointer,
        ) -> Option<Retained<NSAttributedString>> {
            if !actual_range.is_null() {
                // SAFETY: AppKit supplied a writable range pointer for this protocol call.
                let length = self.ivars().marked_text.borrow().encode_utf16().count();
                unsafe {
                    *actual_range = if length == 0 {
                        NSRange::new(NSNotFound as usize, 0)
                    } else {
                        NSRange::new(0, length)
                    }
                };
            }
            let text = self.ivars().marked_text.borrow();
            (!text.is_empty()).then(|| NSAttributedString::from_nsstring(&NSString::from_str(&text)))
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
                // SAFETY: AppKit supplied a writable range pointer for this protocol call.
                unsafe { *actual_range = range };
            }
            let cursor = self.ivars().snapshot.borrow().cursor;
            let cell_width = self.ivars().cell_width.get();
            let cell_height = self.ivars().cell_height.get();
            let local = cursor.map_or_else(
                || NSRect::new(NSPoint::ZERO, NSSize::new(cell_width, cell_height)),
                |cursor| {
                    NSRect::new(
                        NSPoint::new(
                            cursor.column as f64 * cell_width,
                            cursor.line as f64 * cell_height,
                        ),
                        NSSize::new(cell_width, cell_height),
                    )
                },
            );
            let window_rect = self.convertRect_toView(local, None);
            self.window()
                .map_or(window_rect, |window| window.convertRectToScreen(window_rect))
        }

        #[unsafe(method(characterIndexForPoint:))]
        fn character_index_for_point(&self, _point: NSPoint) -> NSUInteger {
            0
        }
    }

    impl TerminalMetalView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(mouseDownCanMoveWindow))]
        fn mouse_down_can_move_window(&self) -> bool {
            false
        }

        #[unsafe(method(becomeFirstResponder))]
        fn become_first_responder(&self) -> bool {
            self.notify_interaction();
            self.ivars().focused.set(true);
            self.ivars().cursor_visible.set(true);
            if let Some(session) = self.ivars().session.borrow().as_ref() {
                session.report_focus(true);
            }
            self.render_frame();
            true
        }

        #[unsafe(method(resignFirstResponder))]
        fn resign_first_responder(&self) -> bool {
            self.ivars().focused.set(false);
            self.ivars().marked_text.borrow_mut().clear();
            if let Some(session) = self.ivars().session.borrow().as_ref() {
                session.report_focus(false);
            }
            self.render_frame();
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty_rect: NSRect) {
            self.render_frame();
        }

        #[unsafe(method(viewDidMoveToWindow))]
        fn view_did_move_to_window(&self) {
            if let Some(window) = self.window() {
                self.ivars().window_occluded.set(
                    !window
                        .occlusionState()
                        .contains(NSWindowOcclusionState::Visible),
                );
                window.setAcceptsMouseMovedEvents(true);
                self.initialize_display_link();
                self.resize_terminal();
                self.render_frame();
            } else if let Some(display_link) = self.ivars().display_link.borrow_mut().take() {
                display_link.invalidate();
                log::debug!("native terminal display link invalidated after leaving its window");
            }
        }

        #[unsafe(method(viewDidChangeBackingProperties))]
        fn view_did_change_backing_properties(&self) {
            self.resize_terminal();
            self.render_frame();
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: NSSize) {
            // SAFETY: Forwarding to NSView preserves AppKit layout bookkeeping.
            unsafe {
                let _: () = msg_send![super(self), setFrameSize: size];
            }
            self.resize_terminal();
            self.render_frame();
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.notify_interaction();
            if let Some(window) = self.window() {
                window.makeFirstResponder(Some(self));
            }
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            let (line, column, side) = self.terminal_grid_point(point);
            if event.clickCount() == 1
                && event
                    .modifierFlags()
                    .contains(NSEventModifierFlags::Command)
                && let Some(target) = terminal_link_at(&self.ivars().snapshot.borrow(), line, column)
            {
                if let Some(delegate) = self.ivars().activation_delegate.borrow().load() {
                    delegate.activate_native_terminal_link(&target);
                }
                return;
            }
            if self.report_mouse_event(
                TerminalMouseButton::Left,
                TerminalMouseAction::Press,
                event,
            ) {
                self.ivars()
                    .reported_mouse_button
                    .set(Some(TerminalMouseButton::Left));
                return;
            }
            let selection_type = if event.clickCount() >= 3 {
                TerminalSelectionType::Lines
            } else if event.clickCount() == 2 {
                TerminalSelectionType::Semantic
            } else if event.modifierFlags().contains(NSEventModifierFlags::Option) {
                TerminalSelectionType::Block
            } else {
                TerminalSelectionType::Simple
            };
            if let Some(session) = self.ivars().session.borrow().as_ref() {
                session.begin_selection(line, column, selection_type, side);
                self.ivars().dragging_selection.set(true);
            }
            self.update_snapshot();
            self.render_frame();
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            if let Some(button) = self.ivars().reported_mouse_button.get() {
                self.report_mouse_event(button, TerminalMouseAction::Move, event);
                return;
            }
            if !self.ivars().dragging_selection.get() {
                return;
            }
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            let (line, column, side) = self.terminal_grid_point(point);
            if let Some(session) = self.ivars().session.borrow().as_ref() {
                session.update_selection(line, column, side);
            }
            self.update_snapshot();
            self.render_frame();
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            if let Some(button) = self.ivars().reported_mouse_button.replace(None) {
                self.report_mouse_event(button, TerminalMouseAction::Release, event);
                return;
            }
            self.ivars().dragging_selection.set(false);
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            self.report_mouse_event(
                TerminalMouseButton::None,
                TerminalMouseAction::Move,
                event,
            );
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &NSEvent) {
            if self.report_mouse_event(
                TerminalMouseButton::Right,
                TerminalMouseAction::Press,
                event,
            ) {
                self.ivars()
                    .reported_mouse_button
                    .set(Some(TerminalMouseButton::Right));
            } else {
                // SAFETY: Forwarding an unreported right click preserves NSView's standard
                // context-menu dispatch through menuForEvent:.
                unsafe {
                    let _: () = msg_send![super(self), rightMouseDown: event];
                }
            }
        }

        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, event: &NSEvent) {
            if self.ivars().reported_mouse_button.get() == Some(TerminalMouseButton::Right) {
                self.ivars().reported_mouse_button.set(None);
                self.report_mouse_event(
                    TerminalMouseButton::Right,
                    TerminalMouseAction::Release,
                    event,
                );
            }
        }

        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, event: &NSEvent) {
            if self.report_mouse_event(
                TerminalMouseButton::Middle,
                TerminalMouseAction::Press,
                event,
            ) {
                self.ivars()
                    .reported_mouse_button
                    .set(Some(TerminalMouseButton::Middle));
            }
        }

        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, event: &NSEvent) {
            if self.ivars().reported_mouse_button.get() == Some(TerminalMouseButton::Middle) {
                self.ivars().reported_mouse_button.set(None);
                self.report_mouse_event(
                    TerminalMouseButton::Middle,
                    TerminalMouseAction::Release,
                    event,
                );
            }
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            self.notify_interaction();
            let delta = event.scrollingDeltaY();
            if delta != 0.0
                && self.report_mouse_event(
                    if delta > 0.0 {
                        TerminalMouseButton::WheelUp
                    } else {
                        TerminalMouseButton::WheelDown
                    },
                    TerminalMouseAction::Press,
                    event,
                )
            {
                return;
            }
            let lines = if event.hasPreciseScrollingDeltas() {
                (delta / self.ivars().cell_height.get()).round() as i32
            } else {
                delta.round() as i32
            };
            if lines != 0
                && let Some(session) = self.ivars().session.borrow().as_ref()
            {
                session.scroll(TerminalScroll::Lines(lines));
                self.update_snapshot();
                self.render_frame();
            }
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            if self.ivars().exited.get() && matches!(event.keyCode(), 36 | 76) {
                if let Some(delegate) = self.ivars().activation_delegate.borrow().load() {
                    delegate.close_exited_native_terminal(self.ivars().session_id.get());
                }
                return;
            }
            self.notify_interaction();
            let modifiers = event.modifierFlags();
            let unmodified = event
                .charactersIgnoringModifiers()
                .map(|text| text.to_string())
                .unwrap_or_default();
            if modifiers.contains(NSEventModifierFlags::Command) {
                let font_delta = match (unmodified.as_str(), event.keyCode()) {
                    ("=" | "+", _) | (_, 69) => Some(1.0),
                    ("-" | "_", _) | (_, 78) => Some(-1.0),
                    _ => None,
                };
                if let Some(delta) = font_delta
                    && !modifiers.intersects(
                        NSEventModifierFlags::Control | NSEventModifierFlags::Option,
                    )
                {
                    if let Some(delegate) = self.ivars().activation_delegate.borrow().load() {
                        delegate.adjust_native_terminal_font_size(delta);
                    }
                    return;
                }
                match unmodified.as_str() {
                    "c" => {
                        self.copy_selection();
                        return;
                    }
                    "v" => {
                        self.paste_clipboard();
                        return;
                    }
                    "a" => {
                        self.select_all();
                        return;
                    }
                    _ => return,
                }
            }
            if modifiers.contains(NSEventModifierFlags::Control)
                && let Some(character) = unmodified.chars().next()
                && character.is_ascii()
            {
                self.send_input(vec![(character.to_ascii_uppercase() as u8) & 0x1f]);
                return;
            }
            if let Some(sequence) = terminal_special_key(event.keyCode()) {
                self.send_input(sequence.as_bytes().to_vec());
                return;
            }
            self.interpretKeyEvents(&NSArray::from_slice(&[event]));
        }

        #[unsafe(method_id(menuForEvent:))]
        fn menu_for_event(&self, _event: &NSEvent) -> Option<Retained<NSMenu>> {
            let menu = NSMenu::new(self.mtm());
            for (title, action, key) in [
                ("Copy", sel!(copy:), "c"),
                ("Copy Screen", sel!(copyScreen:), ""),
                ("Copy All", sel!(copyAll:), ""),
            ] {
                let item = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(self.mtm()),
                        &NSString::from_str(title),
                        Some(action),
                        &NSString::from_str(key),
                    )
                };
                unsafe { item.setTarget(Some(self)) };
                if title == "Copy" {
                    item.setEnabled(
                        self.ivars()
                            .session
                            .borrow()
                            .as_ref()
                            .and_then(TerminalSession::selected_text)
                            .is_some_and(|text| !text.is_empty()),
                    );
                }
                menu.addItem(&item);
            }
            menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
            for (title, action, key) in [
                ("Select All", sel!(selectAll:), "a"),
                ("Paste", sel!(paste:), "v"),
            ] {
                let item = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(self.mtm()),
                        &NSString::from_str(title),
                        Some(action),
                        &NSString::from_str(key),
                    )
                };
                unsafe { item.setTarget(Some(self)) };
                menu.addItem(&item);
            }
            Some(menu)
        }

        #[unsafe(method(copy:))]
        fn copy(&self, _sender: &AnyObject) {
            self.copy_selection();
        }

        #[unsafe(method(copyScreen:))]
        fn copy_screen(&self, _sender: &AnyObject) {
            let text = self
                .ivars()
                .session
                .borrow()
                .as_ref()
                .map(TerminalSession::visible_text);
            if let Some(text) = text.filter(|text| !text.is_empty()) {
                self.store_clipboard(TerminalClipboard::Clipboard, &text);
            }
        }

        #[unsafe(method(copyAll:))]
        fn copy_all(&self, _sender: &AnyObject) {
            let text = self
                .ivars()
                .session
                .borrow()
                .as_ref()
                .map(TerminalSession::all_text);
            if let Some(text) = text.filter(|text| !text.is_empty()) {
                self.store_clipboard(TerminalClipboard::Clipboard, &text);
            }
        }

        #[unsafe(method(paste:))]
        fn paste(&self, _sender: &AnyObject) {
            self.paste_clipboard();
        }

        #[unsafe(method(selectAll:))]
        fn select_all_action(&self, _sender: &AnyObject) {
            self.select_all();
        }

        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(
            &self,
            info: &ProtocolObject<dyn NSDraggingInfo>,
        ) -> NSDragOperation {
            let types = NSArray::from_slice(&[unsafe { NSPasteboardTypeFileURL }]);
            if info
                .draggingPasteboard()
                .availableTypeFromArray(&types)
                .is_some()
            {
                NSDragOperation::Copy
            } else {
                NSDragOperation::None
            }
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag_operation(
            &self,
            info: &ProtocolObject<dyn NSDraggingInfo>,
        ) -> objc2::runtime::Bool {
            let classes = NSArray::from_slice(&[NSURL::class()]);
            let Some(objects) = (unsafe {
                info.draggingPasteboard()
                    .readObjectsForClasses_options(&classes, None)
            }) else {
                return false.into();
            };
            let paths = objects
                .iter()
                .filter_map(|object| object.downcast::<NSURL>().ok())
                .filter(|url| url.isFileURL())
                .filter_map(|url| url.path())
                .map(|path| PathBuf::from(path.to_string()))
                .collect::<Vec<_>>();
            if paths.is_empty() {
                return false.into();
            }
            if let Some(delegate) = self.ivars().activation_delegate.borrow().load()
                && delegate.native_terminal_files_dropped(
                    self.ivars().session_id.get(),
                    paths.clone(),
                )
            {
                return true.into();
            }
            self.paste_file_paths(&paths);
            log::info!("native terminal file drop pasted shell-quoted paths");
            true.into()
        }
    }
);

impl TerminalMetalView {
    pub fn new(frame: NSRect, font_size: f64, mtm: MainThreadMarker) -> Retained<Self> {
        let (font_size, cell_width, cell_height) = terminal_metrics(font_size);
        let this = Self::alloc(mtm).set_ivars(TerminalMetalViewIvars {
            metal: RefCell::new(None),
            session: RefCell::new(None),
            snapshot: RefCell::new(TerminalSnapshot {
                columns: 2,
                lines: 1,
                display_offset: 0,
                cursor: None,
                cells: Vec::new(),
                images: Vec::new(),
            }),
            display_link: RefCell::new(None),
            window_occluded: Cell::new(false),
            dragging_selection: Cell::new(false),
            reported_mouse_button: Cell::new(None),
            focused: Cell::new(false),
            cursor_visible: Cell::new(true),
            blink_timestamp: Cell::new(0.0),
            frame_dirty: Cell::new(true),
            paint_cache: RefCell::new(TerminalPaintCache::new(font_size as f32)),
            accessibility_timestamp: Cell::new(None),
            accessibility_dirty: Cell::new(true),
            marked_text: RefCell::new(String::new()),
            title: RefCell::new("Terminal".to_owned()),
            title_button: RefCell::new(None),
            activation_delegate: RefCell::new(Weak::default()),
            session_id: Cell::new(0),
            exited: Cell::new(false),
            exit_code: Cell::new(None),
            font_size: Cell::new(font_size),
            cell_width: Cell::new(cell_width),
            cell_height: Cell::new(cell_height),
        });
        // SAFETY: NSView's designated frame initializer is valid for this subclass.
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        view.initialize_metal();
        view.setAccessibilityRole(Some(unsafe { NSAccessibilityTextAreaRole }));
        view.setAccessibilityLabel(Some(&NSString::from_str("Terminal")));
        view.registerForDraggedTypes(&NSArray::from_slice(&[unsafe { NSPasteboardTypeFileURL }]));
        view
    }

    pub fn spawn(
        &self,
        program: String,
        arguments: Vec<String>,
        working_directory: Option<PathBuf>,
        title: &str,
    ) -> Result<(), String> {
        self.shutdown()?;
        let viewport = self.viewport();
        let session = TerminalSession::spawn(
            TerminalSpawnOptions {
                working_directory,
                shell_program: Some(program),
                shell_arguments: arguments,
                environment: Default::default(),
            },
            viewport,
        )
        .map_err(|error| format!("Unable to start terminal: {error}"))?;
        log::info!(
            "native terminal session started pid={} columns={} lines={}",
            session.child_pid(),
            viewport.columns,
            viewport.lines
        );
        self.ivars().snapshot.replace(session.snapshot());
        self.ivars().session.replace(Some(session));
        self.set_session_title(title);
        self.ivars().exited.set(false);
        self.ivars().exit_code.set(None);
        self.ivars().cursor_visible.set(true);
        self.setAccessibilityLabel(Some(&NSString::from_str(title)));
        self.start_display_link();
        self.render_frame();
        Ok(())
    }

    pub fn attach_title_button(&self, button: &NSButton) {
        self.ivars().title_button.replace(Some(button.retain()));
        self.refresh_title_presenters();
    }

    pub fn attach_activation_delegate(&self, delegate: &AppDelegate, session_id: isize) {
        self.ivars()
            .activation_delegate
            .replace(Weak::new(delegate));
        self.ivars().session_id.set(session_id);
    }

    pub fn has_session(&self) -> bool {
        self.ivars().session.borrow().is_some()
    }

    pub fn is_active(&self) -> bool {
        self.has_session() && !self.ivars().exited.get()
    }

    pub fn child_pid(&self) -> Option<u32> {
        self.ivars()
            .session
            .borrow()
            .as_ref()
            .map(|session| session.child_pid())
    }

    pub fn exited_successfully(&self) -> bool {
        self.ivars().exited.get() && self.ivars().exit_code.get() == Some(0)
    }

    pub fn is_focused(&self) -> bool {
        self.ivars().focused.get()
    }

    pub fn set_font_size(&self, font_size: f64) {
        let (font_size, cell_width, cell_height) = terminal_metrics(font_size);
        if (self.ivars().font_size.get() - font_size).abs() < f64::EPSILON {
            return;
        }
        self.ivars().font_size.set(font_size);
        self.ivars().cell_width.set(cell_width);
        self.ivars().cell_height.set(cell_height);
        self.resize_terminal();
        self.render_frame();
        log::debug!(
            "native terminal font metrics updated font_size={font_size} cell_width={cell_width} cell_height={cell_height}"
        );
    }

    pub fn search(
        &self,
        pattern: &str,
        direction: TerminalSearchDirection,
    ) -> Result<bool, String> {
        let found = self
            .ivars()
            .session
            .borrow()
            .as_ref()
            .ok_or_else(|| "No terminal session is active.".to_string())?
            .search(pattern, direction)?
            .is_some();
        self.update_snapshot();
        self.render_frame();
        Ok(found)
    }

    pub fn clear_search(&self) {
        if let Some(session) = self.ivars().session.borrow().as_ref() {
            session.clear_selection();
        }
        self.update_snapshot();
        self.render_frame();
    }

    pub(crate) fn paste_file_paths(&self, paths: &[PathBuf]) {
        let text = paths
            .iter()
            .map(|path| format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'")))
            .collect::<Vec<_>>()
            .join(" ");
        if !text.is_empty() {
            self.send_input(text.into_bytes());
        }
    }

    pub fn shutdown(&self) -> Result<(), String> {
        let Some(session) = self.ivars().session.borrow_mut().take() else {
            return Ok(());
        };
        let pid = session.child_pid();
        let result = session
            .shutdown()
            .map_err(|error| format!("Unable to stop terminal: {error}"));
        self.ivars().marked_text.borrow_mut().clear();
        self.ivars().exited.set(true);
        self.ivars().snapshot.borrow_mut().cells.clear();
        log::info!(
            "native terminal session stopped pid={pid} status={}",
            if result.is_ok() { "ok" } else { "error" }
        );
        self.render_frame();
        result
    }

    pub fn teardown_renderer(&self) {
        if let Some(display_link) = self.ivars().display_link.borrow_mut().take() {
            display_link.setPaused(true);
            display_link.setDelegate(None);
            display_link.invalidate();
        }
        self.setLayer(None);
        self.setWantsLayer(false);
        self.ivars().metal.borrow_mut().take();
    }

    pub fn set_window_occluded(&self, occluded: bool) {
        if self.ivars().window_occluded.replace(occluded) == occluded {
            return;
        }
        self.refresh_renderer_visibility();
        log::debug!("native terminal renderer occluded={occluded}");
    }

    pub fn refresh_renderer_visibility(&self) {
        if self.can_render() && self.has_session() {
            self.start_display_link();
        } else if let Some(display_link) = self.ivars().display_link.borrow().as_ref() {
            display_link.setPaused(true);
        }
    }

    pub fn focus_terminal(&self) {
        if let Some(window) = self.window() {
            window.makeFirstResponder(Some(self));
        }
    }

    fn initialize_metal(&self) {
        let Some(device) = MTLCreateSystemDefaultDevice() else {
            log::error!("Metal is unavailable; native terminal cannot be created");
            return;
        };
        let Some(command_queue) = device.newCommandQueue() else {
            log::error!("Metal command queue creation failed for native terminal");
            return;
        };
        let layer = CAMetalLayer::new();
        layer.setDevice(Some(&device));
        layer.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        layer.setPresentsWithTransaction(false);
        layer.setFramebufferOnly(false);
        layer.setMaximumDrawableCount(3);
        let backend = unsafe {
            mtl::BackendContext::new(
                Retained::as_ptr(&device) as mtl::Handle,
                Retained::as_ptr(&command_queue) as mtl::Handle,
            )
        };
        let Some(skia) = gpu::direct_contexts::make_metal(&backend, None) else {
            log::error!("Skia Metal direct context creation failed for native terminal");
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
        log::info!("native Skia Metal terminal surface initialized");
    }

    fn initialize_display_link(&self) {
        if self.ivars().display_link.borrow().is_some() || self.window().is_none() {
            return;
        }
        let Some(layer) = self
            .ivars()
            .metal
            .borrow()
            .as_ref()
            .map(|metal| metal.layer.clone())
        else {
            return;
        };
        let display_link =
            CAMetalDisplayLink::initWithMetalLayer(CAMetalDisplayLink::alloc(), &layer);
        display_link.setDelegate(Some(ProtocolObject::from_ref(self)));
        display_link.setPreferredFrameLatency(2.0);
        display_link.setPaused(!self.has_session() || !self.can_render());
        let main_run_loop = NSRunLoop::mainRunLoop();
        unsafe {
            display_link.addToRunLoop_forMode(&main_run_loop, NSRunLoopCommonModes);
            display_link.addToRunLoop_forMode(&main_run_loop, NSEventTrackingRunLoopMode);
        }
        self.ivars().display_link.replace(Some(display_link));
    }

    fn start_display_link(&self) {
        if !self.can_render() {
            return;
        }
        if self.ivars().display_link.borrow().is_none() {
            self.initialize_display_link();
        }
        if let Some(display_link) = self.ivars().display_link.borrow().as_ref() {
            display_link.setPaused(false);
        }
    }

    fn viewport(&self) -> TerminalViewport {
        let bounds = self.bounds();
        let cell_width = self.ivars().cell_width.get();
        let cell_height = self.ivars().cell_height.get();
        TerminalViewport::new(
            (bounds.size.width / cell_width).floor().max(2.0) as usize,
            (bounds.size.height / cell_height).floor().max(1.0) as usize,
            cell_width.round() as u16,
            cell_height.round() as u16,
        )
    }

    fn resize_terminal(&self) {
        let viewport = self.viewport();
        if let Some(session) = self.ivars().session.borrow_mut().as_mut()
            && let Err(error) = session.resize(viewport)
        {
            log::warn!("native terminal resize failed: {error}");
        }
    }

    fn drain_terminal_events(&self) {
        let batch = self.ivars().session.borrow().as_ref().map(|session| {
            session.drain_events_with_clipboard(|| {
                NSPasteboard::generalPasteboard()
                    .stringForType(unsafe { NSPasteboardTypeString })
                    .map(|text| text.to_string())
            })
        });
        let Some(batch) = batch else { return };
        if let Some((kind, text)) = batch.clipboard_store {
            self.store_clipboard(kind, &text);
        }
        if let Some(title) = batch.title {
            let title = title
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| "Terminal".to_owned());
            match title.strip_prefix(REPORTED_ACTIVITY_TITLE_PREFIX) {
                Some("active") | Some("idle") => {
                    if let Some(delegate) = self.ivars().activation_delegate.borrow().load() {
                        delegate.native_terminal_reported_activity_changed(
                            self.ivars().session_id.get(),
                            title.ends_with("active"),
                        );
                    }
                }
                _ => self.set_session_title(&title),
            }
        }
        if batch.bell {
            objc2_app_kit::NSBeep();
        }
        if batch.exited {
            let first_exit = !self.ivars().exited.replace(true);
            let previous_code = self.ivars().exit_code.get();
            let effective_code = batch.exit_code.or(previous_code);
            self.ivars().exit_code.set(effective_code);
            self.refresh_title_presenters();
            let status_became_known = previous_code.is_none() && batch.exit_code.is_some();
            if (first_exit || status_became_known)
                && let Some(delegate) = self.ivars().activation_delegate.borrow().load()
            {
                delegate
                    .native_terminal_session_exited(self.ivars().session_id.get(), effective_code);
            }
            log::info!("native terminal child exited status={effective_code:?}");
        }
        if batch.needs_redraw || batch.exited {
            self.update_snapshot();
        }
    }

    fn update_snapshot(&self) {
        if let Some(session) = self.ivars().session.borrow().as_ref() {
            let snapshot = session.snapshot();
            self.ivars().snapshot.replace(snapshot);
            self.ivars().accessibility_dirty.set(true);
            self.refresh_accessibility(self.ivars().exited.get());
            self.ivars().frame_dirty.set(true);
        }
    }

    fn refresh_accessibility(&self, force: bool) {
        if !self.ivars().accessibility_dirty.get() {
            return;
        }
        let now = Instant::now();
        if !force
            && self
                .ivars()
                .accessibility_timestamp
                .get()
                .is_some_and(|last| now.duration_since(last) < Duration::from_millis(120))
        {
            return;
        }
        let accessible = terminal_accessibility_text(&self.ivars().snapshot.borrow());
        unsafe { self.setAccessibilityValue(Some(&NSString::from_str(&accessible))) };
        self.ivars().accessibility_timestamp.set(Some(now));
        self.ivars().accessibility_dirty.set(false);
    }

    fn render_frame(&self) {
        if !self.can_render() || !self.has_session() {
            return;
        }
        self.ivars().frame_dirty.set(true);
        self.start_display_link();
    }

    fn can_render(&self) -> bool {
        self.window().is_some()
            && !self.ivars().window_occluded.get()
            && !self.isHiddenOrHasHiddenAncestor()
    }

    fn render_drawable(
        &self,
        drawable: &ProtocolObject<dyn CAMetalDrawable>,
        present_with_command_buffer: bool,
    ) {
        let bounds = self.bounds();
        if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
            return;
        }
        let backing = self.convertRectToBacking(bounds);
        let pixel_width = backing.size.width.max(1.0).round() as i32;
        let pixel_height = backing.size.height.max(1.0).round() as i32;
        let scale_x = pixel_width as f32 / bounds.size.width as f32;
        let scale_y = pixel_height as f32 / bounds.size.height as f32;
        let mut metal_ref = self.ivars().metal.borrow_mut();
        let Some(metal) = metal_ref.as_mut() else {
            return;
        };
        metal.layer.setContentsScale(scale_x as f64);
        metal
            .layer
            .setDrawableSize(CGSize::new(pixel_width as f64, pixel_height as f64));
        let texture = drawable.texture();
        let texture_info =
            unsafe { mtl::TextureInfo::new(Retained::as_ptr(&texture) as mtl::Handle) };
        let backend_target =
            backend_render_targets::make_mtl((pixel_width, pixel_height), &texture_info);
        let Some(mut surface) = gpu::surfaces::wrap_backend_render_target(
            &mut metal.skia,
            &backend_target,
            SurfaceOrigin::TopLeft,
            ColorType::BGRA8888,
            None,
            None,
        ) else {
            log::error!("Skia failed to wrap the native terminal Metal drawable");
            return;
        };
        let canvas = surface.canvas();
        canvas.scale((scale_x, scale_y));
        let snapshot = self.ivars().snapshot.borrow();
        let marked_text = self.ivars().marked_text.borrow();
        paint_terminal(
            canvas,
            TerminalPaintRequest {
                snapshot: &snapshot,
                viewport_width: bounds.size.width as f32,
                viewport_height: bounds.size.height as f32,
                cell_width: self.ivars().cell_width.get() as f32,
                cell_height: self.ivars().cell_height.get() as f32,
                font_size: self.ivars().font_size.get() as f32,
                cursor_visible: self.ivars().cursor_visible.get(),
                focused: self.ivars().focused.get(),
                marked_text: Some(&marked_text),
            },
            &mut self.ivars().paint_cache.borrow_mut(),
        );
        metal.skia.flush_and_submit();
        drop(marked_text);
        drop(snapshot);
        drop(surface);
        let Some(command_buffer) = metal.command_queue.commandBuffer() else {
            return;
        };
        if present_with_command_buffer {
            let presentable: &ProtocolObject<dyn MTLDrawable> = drawable.as_ref();
            command_buffer.presentDrawable(presentable);
            command_buffer.commit();
        } else {
            command_buffer.commit();
            drawable.present();
        }
    }

    fn send_input(&self, bytes: Vec<u8>) {
        if let Some(session) = self.ivars().session.borrow().as_ref() {
            if let Err(error) = session.input(bytes) {
                log::warn!("native terminal input failed: {error}");
            } else {
                session.scroll(TerminalScroll::Bottom);
            }
        }
        self.ivars().cursor_visible.set(true);
        self.start_display_link();
    }

    fn copy_selection(&self) {
        let text = self
            .ivars()
            .session
            .borrow()
            .as_ref()
            .and_then(TerminalSession::selected_text);
        if let Some(text) = text.filter(|text| !text.is_empty()) {
            self.store_clipboard(TerminalClipboard::Clipboard, &text);
        }
    }

    fn paste_clipboard(&self) {
        if let Some(text) =
            NSPasteboard::generalPasteboard().stringForType(unsafe { NSPasteboardTypeString })
        {
            self.send_input(text.to_string().into_bytes());
        }
    }

    fn select_all(&self) {
        let snapshot = self.ivars().snapshot.borrow();
        let session_ref = self.ivars().session.borrow();
        let Some(session) = session_ref.as_ref() else {
            return;
        };
        session.begin_selection(0, 0, TerminalSelectionType::Simple, TerminalSide::Left);
        session.update_selection(
            snapshot.lines.saturating_sub(1),
            snapshot.columns.saturating_sub(1),
            TerminalSide::Right,
        );
        drop(snapshot);
        self.update_snapshot();
        self.render_frame();
    }

    fn store_clipboard(&self, _kind: TerminalClipboard, text: &str) {
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        pasteboard.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString });
    }

    fn set_session_title(&self, title: &str) {
        if self.ivars().title.borrow().as_str() == title {
            return;
        }
        self.ivars().title.replace(title.to_owned());
        self.setAccessibilityLabel(Some(&NSString::from_str(title)));
        self.refresh_title_presenters();
        if let Some(delegate) = self.ivars().activation_delegate.borrow().load() {
            delegate.native_terminal_session_title_changed(self.ivars().session_id.get(), title);
        }
    }

    fn refresh_title_presenters(&self) {
        let title = self.ivars().title.borrow();
        if let Some(button) = self.ivars().title_button.borrow().as_ref() {
            let presented = if self.ivars().exited.get() {
                match self.ivars().exit_code.get() {
                    Some(code) => format!("{title} — exited {code}"),
                    None => format!("{title} — exited"),
                }
            } else {
                title.clone()
            };
            button.setTitle(&NSString::from_str(&presented));
            button.setToolTip(Some(&NSString::from_str(&presented)));
        }
    }

    fn notify_interaction(&self) {
        if let Some(delegate) = self.ivars().activation_delegate.borrow().load() {
            delegate.native_terminal_session_interacted(self.ivars().session_id.get());
        }
    }

    fn report_mouse_event(
        &self,
        button: TerminalMouseButton,
        action: TerminalMouseAction,
        event: &NSEvent,
    ) -> bool {
        let flags = event.modifierFlags();
        if flags.intersects(
            NSEventModifierFlags::Command
                | NSEventModifierFlags::Option
                | NSEventModifierFlags::Shift,
        ) {
            return false;
        }
        let point = self.convertPoint_fromView(event.locationInWindow(), None);
        let (line, column, _) = self.terminal_grid_point(point);
        self.ivars()
            .session
            .borrow()
            .as_ref()
            .is_some_and(|session| {
                session.report_mouse(
                    button,
                    action,
                    TerminalMouseModifiers {
                        shift: false,
                        option: false,
                        control: flags.contains(NSEventModifierFlags::Control),
                    },
                    line,
                    column,
                )
            })
    }

    fn terminal_grid_point(&self, point: NSPoint) -> (usize, usize, TerminalSide) {
        let cell_width = self.ivars().cell_width.get();
        let cell_height = self.ivars().cell_height.get();
        let x = point.x.max(0.0);
        let side = if x % cell_width < cell_width / 2.0 {
            TerminalSide::Left
        } else {
            TerminalSide::Right
        };
        (
            (point.y.max(0.0) / cell_height).floor() as usize,
            (x / cell_width).floor() as usize,
            side,
        )
    }
}

fn terminal_metrics(font_size: f64) -> (f64, f64, f64) {
    let font_size = font_size.clamp(8.0, 32.0);
    let scale = font_size / BASE_FONT_SIZE;
    (
        font_size,
        (BASE_CELL_WIDTH * scale).max(1.0),
        (BASE_CELL_HEIGHT * scale).max(1.0),
    )
}

fn terminal_link_at(snapshot: &TerminalSnapshot, line: usize, column: usize) -> Option<String> {
    if line >= snapshot.lines || column >= snapshot.columns {
        return None;
    }
    if let Some(link) = snapshot
        .cells
        .iter()
        .find(|cell| cell.line == line && cell.column == column)
        .and_then(|cell| cell.hyperlink.as_ref())
    {
        return Some(link.clone());
    }

    let mut characters = vec![' '; snapshot.columns];
    for cell in snapshot.cells.iter().filter(|cell| cell.line == line) {
        if cell.column < characters.len()
            && let Some(character) = cell.text.chars().next()
        {
            characters[cell.column] = character;
        }
    }
    let is_boundary = |character: char| {
        character.is_whitespace()
            || matches!(
                character,
                '\'' | '"' | '<' | '>' | '(' | ')' | '[' | ']' | '{' | '}'
            )
    };
    if is_boundary(characters[column]) {
        return None;
    }
    let mut start = column;
    while start > 0 && !is_boundary(characters[start - 1]) {
        start -= 1;
    }
    let mut end = column + 1;
    while end < characters.len() && !is_boundary(characters[end]) {
        end += 1;
    }
    let candidate = characters[start..end]
        .iter()
        .collect::<String>()
        .trim_matches(|character: char| matches!(character, ',' | ';' | '!' | '?'))
        .trim_end_matches(['.', ':'])
        .to_string();
    looks_like_terminal_link(&candidate).then_some(candidate)
}

fn looks_like_terminal_link(candidate: &str) -> bool {
    candidate.contains("://")
        || candidate.starts_with("mailto:")
        || candidate.starts_with('/')
        || candidate.starts_with("./")
        || candidate.starts_with("../")
        || candidate.contains('/')
}

fn terminal_input_text(string: &AnyObject) -> String {
    if let Some(string) = string.downcast_ref::<NSString>() {
        string.to_string()
    } else if let Some(string) = string.downcast_ref::<NSAttributedString>() {
        string.string().to_string()
    } else {
        String::new()
    }
}

fn terminal_special_key(key_code: u16) -> Option<&'static str> {
    match key_code {
        36 | 76 => Some("\r"),
        48 => Some("\t"),
        51 => Some("\x7f"),
        53 => Some("\x1b"),
        117 => Some("\x1b[3~"),
        123 => Some("\x1b[D"),
        124 => Some("\x1b[C"),
        125 => Some("\x1b[B"),
        126 => Some("\x1b[A"),
        115 => Some("\x1b[H"),
        119 => Some("\x1b[F"),
        116 => Some("\x1b[5~"),
        121 => Some("\x1b[6~"),
        122 => Some("\x1bOP"),
        120 => Some("\x1bOQ"),
        99 => Some("\x1bOR"),
        118 => Some("\x1bOS"),
        _ => None,
    }
}

fn terminal_accessibility_text(snapshot: &TerminalSnapshot) -> String {
    let mut lines = vec![String::new(); snapshot.lines];
    for image in &snapshot.images {
        let Ok(line_index) = usize::try_from(image.line) else {
            continue;
        };
        let Some(line) = lines.get_mut(line_index) else {
            continue;
        };
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&format!(
            "[Sixel image, {} by {} pixels]",
            image.width, image.height
        ));
    }
    for cell in &snapshot.cells {
        let Some(line) = lines.get_mut(cell.line) else {
            continue;
        };
        while line.chars().count() < cell.column {
            line.push(' ');
        }
        line.push_str(&cell.text);
    }
    lines
        .into_iter()
        .map(|line| line.trim_end().to_owned())
        .collect::<Vec<_>>()
        .join("\n")
}
