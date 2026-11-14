fn native_agent_provider_icon(provider_id: &str) -> Option<Retained<NSImage>> {
    let svg: &[u8] = match provider_id {
        "codex" => include_bytes!("../../../craic-ui-core/assets/icons/craic-codex-symbolic.svg"),
        "grok" => include_bytes!("../../../craic-ui-core/assets/icons/craic-grok-symbolic.svg"),
        "opencode" => {
            include_bytes!("../../../craic-ui-core/assets/icons/opencode-logo-dark-square.svg")
        }
        "agy" => {
            include_bytes!("../../../craic-ui-core/assets/icons/craic-antigravity-symbolic.svg")
        }
        _ => return None,
    };
    // Render the same SVG assets GTK uses, at 2x resolution for Retina menus.
    let mut dom = skia_safe::svg::Dom::from_bytes(svg, skia_safe::FontMgr::empty()).ok()?;
    dom.set_container_size((36.0, 36.0));
    let mut root = dom.root();
    root.set_width(skia_safe::svg::Length::new(
        36.0,
        skia_safe::svg::LengthUnit::Number,
    ));
    root.set_height(skia_safe::svg::Length::new(
        36.0,
        skia_safe::svg::LengthUnit::Number,
    ));
    let mut surface = skia_safe::surfaces::raster_n32_premul((36, 36))?;
    surface.canvas().clear(skia_safe::Color::TRANSPARENT);
    dom.render(surface.canvas());
    let png = surface
        .image_snapshot()
        .encode(None, skia_safe::EncodedImageFormat::PNG, 100)?;
    let image = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(png.as_bytes()))?;
    image.setSize(NSSize::new(18.0, 18.0));
    image.setTemplate(provider_id != "opencode");
    Some(image)
}
