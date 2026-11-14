fn native_page_badge_image(page_id: &str, count: usize) -> Option<Retained<NSImage>> {
    use objc2_app_kit::{
        NSAttributedStringNSStringDrawing, NSBezierPath, NSCompositingOperation, NSGraphicsContext,
    };
    let symbol = match page_id {
        "changes" => "arrow.triangle.branch",
        "agents" => "sparkles",
        _ => return None,
    };
    let base = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(symbol),
        None,
    )?;
    let label = if count > 999 {
        "999+".to_owned()
    } else {
        count.to_string()
    };
    let text = NSMutableAttributedString::initWithString(
        NSMutableAttributedString::alloc(),
        &NSString::from_str(&label),
    );
    let font = NSFont::monospacedDigitSystemFontOfSize_weight(8.0, 0.5);
    // SAFETY: The standard font/color attributes receive the matching AppKit value types.
    unsafe {
        text.addAttribute_value_range(NSFontAttributeName, &font, NSRange::new(0, text.length()));
        text.addAttribute_value_range(
            NSForegroundColorAttributeName,
            &NSColor::blackColor(),
            NSRange::new(0, text.length()),
        );
    }
    let text_size = text.size();
    let badge_width = (text_size.width + 6.0).max(14.0);
    let size = if count == 0 {
        NSSize::new(20.0, 20.0)
    } else {
        NSSize::new(14.0 + badge_width, 24.0)
    };
    let draw = RcBlock::new(move |_rect| {
        base.drawInRect(NSRect::new(NSPoint::ZERO, NSSize::new(20.0, 20.0)));
        if count > 0 {
            let badge = NSRect::new(NSPoint::new(14.0, 10.0), NSSize::new(badge_width, 14.0));
            if let Some(context) = NSGraphicsContext::currentContext() {
                context.saveGraphicsState();
                // A clear outline separates the count circle from the symbol. Knockout
                // numerals preserve native selected/dark/light tinting in a template image.
                context.setCompositingOperation(NSCompositingOperation::Clear);
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                    NSRect::new(
                        NSPoint::new(12.5, 8.5),
                        NSSize::new(badge_width + 3.0, 17.0),
                    ),
                    8.5,
                    8.5,
                )
                .fill();
                context.setCompositingOperation(NSCompositingOperation::SourceOver);
                NSColor::blackColor().setFill();
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(badge, 7.0, 7.0).fill();
                context.setCompositingOperation(NSCompositingOperation::DestinationOut);
                text.drawAtPoint(NSPoint::new(
                    badge.origin.x + (badge_width - text_size.width) / 2.0,
                    badge.origin.y + (14.0 - text_size.height) / 2.0,
                ));
                context.restoreGraphicsState();
            }
        }
        true.into()
    });
    let image = NSImage::imageWithSize_flipped_drawingHandler(size, false, &draw);
    image.setTemplate(true);
    image.setAccessibilityDescription(Some(&NSString::from_str(if page_id == "changes" {
        "Changes"
    } else {
        "Agents"
    })));
    image.setName(Some(&NSString::from_str(&format!(
        "dev.craic.page-badge.{page_id}.{count}"
    ))));
    Some(image)
}
