use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{AnyThread, MainThreadOnly};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBezelStyle, NSBorderType, NSBox, NSBoxType, NSButton, NSColor,
    NSControlBorderShape, NSControlSize, NSFont, NSImage, NSImageScaling, NSImageView,
    NSProgressIndicator, NSProgressIndicatorStyle, NSScrollView, NSTextField, NSTextView,
    NSTrackingArea, NSTrackingAreaOptions, NSView,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};

pub const COMMIT_COMPOSER_HEIGHT: f64 = 228.0;

#[derive(Clone, Copy)]
pub struct CommitComposerActions {
    pub select_author: Sel,
    pub show_author_warning: Sel,
    pub summary_changed: Sel,
    pub generate_message: Sel,
    pub commit: Sel,
}

pub struct CommitComposer {
    pub root: Retained<NSView>,
    pub author_button: Retained<NSButton>,
    author_image: Retained<NSImageView>,
    pub author_warning: Retained<NSButton>,
    pub summary_field: Retained<NSTextField>,
    pub description_view: Retained<NSTextView>,
    pub generate_button: Retained<NSButton>,
    pub commit_button: Retained<NSButton>,
    completion_label: Retained<NSTextField>,
    generate_spinner: Retained<NSProgressIndicator>,
    commit_spinner: Retained<NSProgressIndicator>,
    repository_available: Cell<bool>,
    has_selected_files: Cell<bool>,
    can_generate: Cell<bool>,
    generating: Cell<bool>,
    generation_hovered: Cell<bool>,
    committing: Cell<bool>,
    default_summary: RefCell<Option<String>>,
    author_warning_text: RefCell<Option<String>>,
}

impl CommitComposer {
    pub fn new(
        frame: NSRect,
        target: &AnyObject,
        actions: CommitComposerActions,
        mtm: MainThreadMarker,
    ) -> Self {
        let root = NSView::initWithFrame(NSView::alloc(mtm), frame);
        root.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );

        let width = frame.size.width;
        let separator = NSBox::initWithFrame(
            NSBox::alloc(mtm),
            NSRect::new(
                NSPoint::new(0.0, frame.size.height - 1.0),
                NSSize::new(width, 1.0),
            ),
        );
        separator.setBoxType(NSBoxType::Separator);
        separator.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        root.addSubview(&separator);

        let summary_y = frame.size.height - 45.0;
        let author_image = symbol("person.crop.circle.fill", "Select commit author");
        let author_image_view = NSImageView::imageViewWithImage(&author_image, mtm);
        author_image_view.setFrame(NSRect::new(
            NSPoint::new(12.0, summary_y),
            NSSize::new(34.0, 34.0),
        ));
        author_image_view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
        author_image_view.setWantsLayer(true);
        if let Some(layer) = author_image_view.layer() {
            layer.setCornerRadius(17.0);
            layer.setMasksToBounds(true);
        }
        author_image_view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        root.addSubview(&author_image_view);
        // SAFETY: The integration target implements the supplied select-author selector.
        let author_button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::new(),
                Some(target),
                Some(actions.select_author),
                mtm,
            )
        };
        author_button.setFrame(NSRect::new(
            NSPoint::new(12.0, summary_y),
            NSSize::new(34.0, 34.0),
        ));
        author_button.setBordered(false);
        author_button.setTransparent(true);
        author_button.setControlSize(NSControlSize::Regular);
        author_button.setToolTip(Some(&NSString::from_str("Select commit email")));
        author_button.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        root.addSubview(&author_button);

        let warning_image = symbol("exclamationmark.triangle.fill", "Git author warning");
        // SAFETY: The integration target implements the supplied warning selector.
        let author_warning = unsafe {
            NSButton::buttonWithImage_target_action(
                &warning_image,
                Some(target),
                Some(actions.show_author_warning),
                mtm,
            )
        };
        author_warning.setFrame(NSRect::new(
            NSPoint::new(39.0, summary_y + 22.0),
            NSSize::new(14.0, 14.0),
        ));
        author_warning.setContentTintColor(Some(&NSColor::systemOrangeColor()));
        author_warning.setBordered(false);
        author_warning.setToolTip(Some(&NSString::from_str("Show Git author warning")));
        author_warning.setHidden(true);
        author_warning.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        root.addSubview(&author_warning);

        let generate_image = symbol("wand.and.stars", "Generate commit message");
        // SAFETY: The integration target implements the supplied generate-message selector.
        let generate_button = unsafe {
            NSButton::buttonWithImage_target_action(
                &generate_image,
                Some(target),
                Some(actions.generate_message),
                mtm,
            )
        };
        generate_button.setFrame(NSRect::new(
            NSPoint::new(width - 46.0, summary_y),
            NSSize::new(34.0, 34.0),
        ));
        generate_button.setBezelStyle(NSBezelStyle::Glass);
        generate_button.setBorderShape(NSControlBorderShape::Circle);
        generate_button.setControlSize(NSControlSize::Regular);
        generate_button.setToolTip(Some(&NSString::from_str("Generate commit message")));
        generate_button.setEnabled(false);
        generate_button.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        let generate_tracking = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                NSRect::ZERO,
                NSTrackingAreaOptions::MouseEnteredAndExited
                    | NSTrackingAreaOptions::ActiveInActiveApp
                    | NSTrackingAreaOptions::InVisibleRect,
                Some(target),
                None,
            )
        };
        generate_button.addTrackingArea(&generate_tracking);
        root.addSubview(&generate_button);

        let generate_spinner = NSProgressIndicator::initWithFrame(
            NSProgressIndicator::alloc(mtm),
            generate_button.frame(),
        );
        generate_spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        generate_spinner.setControlSize(NSControlSize::Small);
        generate_spinner.setIndeterminate(true);
        generate_spinner.setDisplayedWhenStopped(false);
        generate_spinner.setHidden(true);
        generate_spinner.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        root.addSubview(&generate_spinner);

        let summary_field = NSTextField::initWithFrame(
            NSTextField::alloc(mtm),
            NSRect::new(
                NSPoint::new(54.0, summary_y),
                NSSize::new(width - 108.0, 34.0),
            ),
        );
        summary_field.setPlaceholderString(Some(&NSString::from_str("Summary (required)")));
        summary_field.setControlSize(NSControlSize::Large);
        summary_field.setFont(Some(&NSFont::systemFontOfSize(13.0)));
        summary_field.sizeToFit();
        let summary_height = summary_field.frame().size.height;
        summary_field.setFrame(NSRect::new(
            NSPoint::new(54.0, summary_y + (34.0 - summary_height) / 2.0),
            NSSize::new(width - 108.0, summary_height),
        ));
        summary_field.setContinuous(true);
        summary_field.setEnabled(false);
        summary_field.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        // SAFETY: The integration target implements the supplied summary-changed selector.
        unsafe {
            summary_field.setTarget(Some(target));
            summary_field.setAction(Some(actions.summary_changed));
        }
        root.addSubview(&summary_field);

        let description_view = NSTextView::initWithFrame(
            NSTextView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width - 24.0, 104.0)),
        );
        description_view.setSelectable(true);
        description_view.setRichText(false);
        description_view.setDrawsBackground(false);
        description_view.setFont(Some(&NSFont::systemFontOfSize(13.0)));
        description_view.setTextContainerInset(NSSize::new(7.0, 7.0));
        description_view.setAutomaticQuoteSubstitutionEnabled(false);
        description_view.setAutomaticDashSubstitutionEnabled(false);
        description_view.setEditable(false);

        let description_scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            NSRect::new(
                NSPoint::new(12.0, 47.0),
                NSSize::new(width - 24.0, frame.size.height - 100.0),
            ),
        );
        description_scroll.setBorderType(NSBorderType::BezelBorder);
        description_scroll.setDrawsBackground(true);
        description_scroll.setHasVerticalScroller(true);
        description_scroll.setAutohidesScrollers(true);
        description_scroll.setDocumentView(Some(&description_view));
        description_scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        root.addSubview(&description_scroll);

        // SAFETY: The integration target implements the supplied commit selector.
        let commit_button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("Commit to branch"),
                Some(target),
                Some(actions.commit),
                mtm,
            )
        };
        commit_button.setFrame(NSRect::new(
            NSPoint::new(12.0, 8.0),
            NSSize::new(width - 24.0, 32.0),
        ));
        commit_button.setBezelStyle(NSBezelStyle::Push);
        commit_button.setBorderShape(NSControlBorderShape::RoundedRectangle);
        commit_button.setKeyEquivalent(&NSString::from_str("\r"));
        commit_button.setEnabled(false);
        commit_button.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        root.addSubview(&commit_button);

        let completion_label = NSTextField::labelWithString(&NSString::new(), mtm);
        completion_label.setFrame(commit_button.frame());
        completion_label.setAlignment(objc2_app_kit::NSTextAlignment::Center);
        completion_label.setFont(Some(&NSFont::systemFontOfSize(12.0)));
        completion_label.setTextColor(Some(&NSColor::secondaryLabelColor()));
        completion_label.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        completion_label.setHidden(true);
        completion_label.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        root.addSubview(&completion_label);

        let commit_spinner = NSProgressIndicator::initWithFrame(
            NSProgressIndicator::alloc(mtm),
            NSRect::new(NSPoint::new(22.0, 16.0), NSSize::new(16.0, 16.0)),
        );
        commit_spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        commit_spinner.setControlSize(NSControlSize::Small);
        commit_spinner.setIndeterminate(true);
        commit_spinner.setDisplayedWhenStopped(false);
        commit_spinner.setHidden(true);
        commit_spinner.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);
        root.addSubview(&commit_spinner);

        Self {
            root,
            author_button,
            author_image: author_image_view,
            author_warning,
            summary_field,
            description_view,
            generate_button,
            commit_button,
            completion_label,
            generate_spinner,
            commit_spinner,
            repository_available: Cell::new(false),
            has_selected_files: Cell::new(false),
            can_generate: Cell::new(false),
            generating: Cell::new(false),
            generation_hovered: Cell::new(false),
            committing: Cell::new(false),
            default_summary: RefCell::new(None),
            author_warning_text: RefCell::new(None),
        }
    }

    pub fn set_branch(&self, branch: Option<&str>) {
        let title = branch
            .filter(|branch| !branch.trim().is_empty())
            .map(|branch| format!("Commit to {branch}"))
            .unwrap_or_else(|| "Commit to branch".to_owned());
        self.commit_button
            .setTitle(&NSString::from_str(title.as_str()));
    }

    pub fn set_repository_available(&self, available: bool) {
        self.repository_available.set(available);
        self.summary_field
            .setEnabled(available && !self.committing.get() && !self.generating.get());
        self.description_view
            .setEditable(available && !self.committing.get() && !self.generating.get());
        self.refresh_generate_state();
        self.refresh_action_state();
    }

    pub fn set_can_generate(&self, can_generate: bool) {
        self.can_generate.set(can_generate);
        self.refresh_generate_state();
    }

    pub fn set_has_selected_files(&self, has_selected_files: bool) {
        self.has_selected_files.set(has_selected_files);
        self.refresh_generate_state();
        self.refresh_action_state();
    }

    pub fn set_default_summary(&self, default_summary: Option<String>) {
        self.summary_field
            .setPlaceholderString(Some(&NSString::from_str(
                default_summary.as_deref().unwrap_or("Summary (required)"),
            )));
        self.default_summary.replace(default_summary);
        self.refresh_action_state();
    }

    pub fn refresh_action_state(&self) {
        let has_summary = !self.summary().trim().is_empty();
        self.commit_button.setEnabled(
            self.repository_available.get()
                && self.has_selected_files.get()
                && has_summary
                && !self.generating.get()
                && !self.committing.get(),
        );
    }

    pub fn set_generating(&self, generating: bool) {
        self.generating.set(generating);
        self.generation_hovered.set(false);
        self.summary_field
            .setEnabled(!generating && !self.committing.get() && self.repository_available.get());
        self.description_view
            .setEditable(!generating && !self.committing.get() && self.repository_available.get());
        self.refresh_generation_presentation();
        self.refresh_generate_state();
        self.refresh_action_state();
    }

    pub fn set_generation_hovered(&self, hovered: bool) {
        if !self.generating.get() || self.generation_hovered.replace(hovered) == hovered {
            return;
        }
        self.refresh_generation_presentation();
    }

    pub fn is_generating(&self) -> bool {
        self.generating.get()
    }

    pub fn set_committing(&self, committing: bool) {
        if committing {
            self.clear_completion();
        }
        self.committing.set(committing);
        self.summary_field
            .setEnabled(!committing && !self.generating.get() && self.repository_available.get());
        self.description_view
            .setEditable(!committing && !self.generating.get() && self.repository_available.get());
        self.commit_spinner.setHidden(!committing);
        if committing {
            // SAFETY: AppKit accepts nil as the sender for programmatic animation changes.
            unsafe { self.commit_spinner.startAnimation(None) };
        } else {
            // SAFETY: AppKit accepts nil as the sender for programmatic animation changes.
            unsafe { self.commit_spinner.stopAnimation(None) };
        }
        self.refresh_generate_state();
        self.refresh_action_state();
    }

    pub fn set_author(&self, name: Option<&str>, image: Option<&NSImage>, warning: Option<&str>) {
        let name = name
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("Git author");
        if let Some(image) = image {
            self.author_image.setImage(Some(image));
        } else {
            self.author_image
                .setImage(Some(&symbol("person.crop.circle.fill", name)));
        }
        self.author_button
            .setToolTip(Some(&NSString::from_str(&format!(
                "{name}\nClick to select commit email"
            ))));
        self.author_warning.setHidden(warning.is_none());
        self.author_warning_text
            .replace(warning.map(ToString::to_string));
    }

    pub fn author_warning_text(&self) -> Option<String> {
        self.author_warning_text.borrow().clone()
    }

    pub fn set_message(&self, summary: &str, description: &str) {
        self.summary_field
            .setStringValue(&NSString::from_str(summary));
        self.description_view
            .setString(&NSString::from_str(description));
        self.refresh_action_state();
    }

    pub fn show_completion(&self, message: &str) {
        let message = message.trim();
        let message = if message.is_empty() {
            "Commit created."
        } else {
            message.lines().next().unwrap_or("Commit created.")
        };
        self.completion_label
            .setStringValue(&NSString::from_str(message));
        self.completion_label
            .setToolTip(Some(&NSString::from_str(message)));
        self.completion_label.setHidden(false);
        self.commit_button.setHidden(true);
    }

    pub fn clear_completion(&self) {
        self.completion_label.setHidden(true);
        self.commit_button.setHidden(false);
    }

    pub fn summary(&self) -> String {
        let summary = self.summary_field.stringValue().to_string();
        if !summary.trim().is_empty() {
            return summary.trim().to_string();
        }
        self.default_summary.borrow().clone().unwrap_or_default()
    }

    pub fn description(&self) -> String {
        self.description_view.string().to_string()
    }

    fn refresh_generate_state(&self) {
        self.generate_button.setEnabled(
            self.generating.get()
                || (self.repository_available.get()
                    && self.has_selected_files.get()
                    && self.can_generate.get()
                    && !self.committing.get()),
        );
    }

    fn refresh_generation_presentation(&self) {
        if !self.generating.get() {
            unsafe { self.generate_spinner.stopAnimation(None) };
            self.generate_spinner.setHidden(true);
            self.generate_button
                .setImage(Some(&symbol("wand.and.stars", "Generate commit message")));
            self.generate_button
                .setToolTip(Some(&NSString::from_str("Generate commit message")));
            return;
        }
        if self.generation_hovered.get() {
            unsafe { self.generate_spinner.stopAnimation(None) };
            self.generate_spinner.setHidden(true);
            self.generate_button
                .setImage(Some(&symbol("xmark", "Cancel commit message generation")));
            self.generate_button.setToolTip(Some(&NSString::from_str(
                "Cancel commit message generation",
            )));
        } else {
            self.generate_button.setImage(None);
            self.generate_button
                .setToolTip(Some(&NSString::from_str("Generating commit message")));
            self.generate_spinner.setHidden(false);
            unsafe { self.generate_spinner.startAnimation(None) };
        }
    }
}

fn symbol(name: &str, accessibility_description: &str) -> Retained<NSImage> {
    NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        Some(&NSString::from_str(accessibility_description)),
    )
    .unwrap_or_else(NSImage::new)
}
