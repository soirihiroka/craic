use gtk::{gdk, gio};

pub(super) struct LoadedImage {
    pub(super) texture: gdk::Texture,
    pub(super) width: i32,
    pub(super) height: i32,
    pub(super) mime_type: String,
}

pub(super) fn load_image(file: gio::File) -> Result<LoadedImage, String> {
    let context = gtk::glib::MainContext::new();
    let load_result = context.block_on(async {
        let loader = glycin::Loader::new(file);
        let image = loader
            .load()
            .await
            .map_err(|err| format!("Unable to load image metadata: {err}"))?;
        let frame = image
            .next_frame()
            .await
            .map_err(|err| format!("Unable to load image frame: {err}"))?;

        Ok::<_, String>((image, frame))
    });

    let (image, frame) = load_result?;
    let width = i32::try_from(frame.width())
        .map_err(|_| "Image width is too large to display.".to_string())?;
    let height = i32::try_from(frame.height())
        .map_err(|_| "Image height is too large to display.".to_string())?;

    if width <= 0 || height <= 0 {
        return Err("Image has zero size.".to_string());
    }

    let mime_type = image.mime_type().to_string();

    Ok(LoadedImage {
        texture: frame.texture(),
        width,
        height,
        mime_type,
    })
}
