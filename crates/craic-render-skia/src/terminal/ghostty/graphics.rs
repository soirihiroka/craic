use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Instant;

use skia_safe::Codec;

use super::*;

handle!(
    ImageIterator,
    GhosttyRenderStateImageIterator,
    ghostty_render_state_image_iterator_free
);

const MAX_IMAGE_BYTES: usize = 10_000_000;

pub(super) struct Graphics {
    iter: ImageIterator,
    images: HashMap<(u32, u64), Image>,
    screen: Option<GhosttyTerminalScreen>,
    next_animation: u64,
}

impl Graphics {
    pub(super) fn new() -> io::Result<Self> {
        static PNG: OnceLock<GhosttyResult> = OnceLock::new();
        check(*PNG.get_or_init(|| unsafe {
            ghostty_sys_set(GHOSTTY_SYS_OPT_DECODE_PNG, decode_png as *const c_void)
        }))?;
        Ok(Self {
            iter: ImageIterator::new(|out| unsafe {
                ghostty_render_state_image_iterator_new(null(), out)
            })?,
            images: HashMap::new(),
            screen: None,
            next_animation: u64::MAX,
        })
    }

    pub(super) fn tick(
        &mut self,
        terminal: GhosttyTerminal,
        output_changed: bool,
    ) -> (bool, Option<Duration>) {
        static START: OnceLock<Instant> = OnceLock::new();
        let now = START.get_or_init(Instant::now).elapsed().as_millis() as u64;
        let frame_due = now >= self.next_animation;
        if output_changed || frame_due {
            self.next_animation = u64::MAX;
            if let Some(graphics) = storage(terminal) {
                unsafe {
                    ghostty_kitty_graphics_animation_tick(graphics, now, &mut self.next_animation);
                }
            }
        }
        let delay = (self.next_animation != u64::MAX)
            .then(|| Duration::from_millis(self.next_animation.saturating_sub(now).max(1)));
        (frame_due, delay)
    }

    pub(super) fn render(
        &mut self,
        terminal: GhosttyTerminal,
        render: GhosttyRenderState,
    ) -> Vec<TerminalImage> {
        let mut images = Vec::new();
        let mut screen = 0;
        if unsafe {
            ghostty_terminal_get(
                terminal,
                GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN,
                out(&mut screen),
            )
        } == GHOSTTY_SUCCESS
            && self.screen.replace(screen) != Some(screen)
        {
            self.images.clear();
        }
        let mut previous = std::mem::take(&mut self.images);
        let Some(storage) = storage(terminal) else {
            return images;
        };
        let mut iter = self.iter.0;
        if unsafe {
            ghostty_render_state_get(
                render,
                GHOSTTY_RENDER_STATE_DATA_IMAGE_ITERATOR,
                out(&mut iter),
            )
        } != GHOSTTY_SUCCESS
        {
            return images;
        }

        while unsafe { ghostty_render_state_image_iterator_next(iter) } {
            let get = |kind, output| unsafe { ghostty_render_state_image_get(iter, kind, output) };
            let mut id = 0u32;
            let mut generation = 0u64;
            let mut layer: GhosttyImageLayer = GHOSTTY_IMAGE_LAYER_BELOW_TEXT;
            let (mut column, mut line) = (0i32, 0i32);
            let (mut x, mut y, mut width, mut height) = (0u32, 0u32, 0u32, 0u32);
            let (mut source_x, mut source_y, mut source_width, mut source_height) =
                (0u32, 0u32, 0u32, 0u32);
            if ![
                (GHOSTTY_RENDER_STATE_IMAGE_DATA_IMAGE_ID, out(&mut id)),
                (
                    GHOSTTY_RENDER_STATE_IMAGE_DATA_IMAGE_GENERATION,
                    out(&mut generation),
                ),
                (GHOSTTY_RENDER_STATE_IMAGE_DATA_LAYER, out(&mut layer)),
                (
                    GHOSTTY_RENDER_STATE_IMAGE_DATA_VIEWPORT_COL,
                    out(&mut column),
                ),
                (GHOSTTY_RENDER_STATE_IMAGE_DATA_VIEWPORT_ROW, out(&mut line)),
                (GHOSTTY_RENDER_STATE_IMAGE_DATA_CELL_OFFSET_X, out(&mut x)),
                (GHOSTTY_RENDER_STATE_IMAGE_DATA_CELL_OFFSET_Y, out(&mut y)),
                (GHOSTTY_RENDER_STATE_IMAGE_DATA_PIXEL_WIDTH, out(&mut width)),
                (
                    GHOSTTY_RENDER_STATE_IMAGE_DATA_PIXEL_HEIGHT,
                    out(&mut height),
                ),
                (GHOSTTY_RENDER_STATE_IMAGE_DATA_SOURCE_X, out(&mut source_x)),
                (GHOSTTY_RENDER_STATE_IMAGE_DATA_SOURCE_Y, out(&mut source_y)),
                (
                    GHOSTTY_RENDER_STATE_IMAGE_DATA_SOURCE_WIDTH,
                    out(&mut source_width),
                ),
                (
                    GHOSTTY_RENDER_STATE_IMAGE_DATA_SOURCE_HEIGHT,
                    out(&mut source_height),
                ),
            ]
            .into_iter()
            .all(|(kind, output)| get(kind, output) == GHOSTTY_SUCCESS)
            {
                continue;
            }
            let key = (id, generation);
            let image = self
                .images
                .get(&key)
                .cloned()
                .or_else(|| previous.remove(&key))
                .or_else(|| raster_image(storage, id));
            let Some(image) = image else { continue };
            self.images.insert(key, image.clone());
            images.push(TerminalImage {
                id: u64::from(id),
                generation,
                line,
                column,
                width: width as usize,
                height: height as usize,
                offset: (x, y),
                source: (source_x, source_y, source_width, source_height),
                layer: match layer {
                    GHOSTTY_IMAGE_LAYER_BELOW_BACKGROUND => ImageLayer::BelowBackground,
                    GHOSTTY_IMAGE_LAYER_ABOVE_TEXT => ImageLayer::AboveText,
                    _ => ImageLayer::BelowText,
                },
                image,
            });
        }
        images
    }
}

fn storage(terminal: GhosttyTerminal) -> Option<GhosttyKittyGraphics> {
    let mut graphics: GhosttyKittyGraphics = null_mut();
    let result = unsafe {
        ghostty_terminal_get(
            terminal,
            GHOSTTY_TERMINAL_DATA_KITTY_GRAPHICS,
            out(&mut graphics),
        )
    };
    (result == GHOSTTY_SUCCESS && !graphics.is_null()).then_some(graphics)
}

fn raster_image(graphics: GhosttyKittyGraphics, id: u32) -> Option<Image> {
    unsafe {
        let image = ghostty_kitty_graphics_image(graphics, id);
        if image.is_null() {
            return None;
        }
        let get = |kind, output| check(ghostty_kitty_graphics_image_get(image, kind, output)).ok();
        let (mut width, mut height) = (0u32, 0u32);
        let mut format: GhosttyKittyImageFormat = GHOSTTY_KITTY_IMAGE_FORMAT_RGBA;
        let mut pixels = GhosttyString::default();
        get(GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_WIDTH, out(&mut width))?;
        get(GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_HEIGHT, out(&mut height))?;
        get(GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_FORMAT, out(&mut format))?;
        get(GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_PIXELS, out(&mut pixels))?;
        let width = i32::try_from(width).ok()?;
        let height = i32::try_from(height).ok()?;
        let count = (width as usize).checked_mul(height as usize)?;
        let channels = match format {
            GHOSTTY_KITTY_IMAGE_FORMAT_RGBA => 4,
            GHOSTTY_KITTY_IMAGE_FORMAT_RGB => 3,
            GHOSTTY_KITTY_IMAGE_FORMAT_GRAY_ALPHA => 2,
            GHOSTTY_KITTY_IMAGE_FORMAT_GRAY => 1,
            _ => return None,
        };
        if count == 0
            || pixels.len != count.checked_mul(channels)?
            || pixels.len > MAX_IMAGE_BYTES
            || pixels.ptr.is_null()
        {
            return None;
        }
        let input = slice::from_raw_parts(pixels.ptr, pixels.len);
        let data = if channels == 4 {
            Data::new_copy(input)
        } else {
            let mut rgba = Vec::with_capacity(count * 4);
            for pixel in input.chunks_exact(channels) {
                let converted = match pixel {
                    [gray] => [*gray, *gray, *gray, 255],
                    [gray, alpha] => [*gray, *gray, *gray, *alpha],
                    [r, g, b] => [*r, *g, *b, 255],
                    _ => return None,
                };
                rgba.extend_from_slice(&converted);
            }
            Data::new_copy(&rgba)
        };
        let info = ImageInfo::new(
            (width, height),
            ColorType::RGBA8888,
            AlphaType::Unpremul,
            None,
        );
        images::raster_from_data(&info, data, width as usize * 4)
    }
}

unsafe extern "C" fn decode_png(
    _: *mut c_void,
    allocator: *const GhosttyAllocator,
    bytes: *const u8,
    len: usize,
    output: *mut GhosttySysImage,
) -> bool {
    if len == 0 || len > MAX_IMAGE_BYTES || bytes.is_null() {
        return false;
    }
    let encoded = unsafe { slice::from_raw_parts(bytes, len) };
    let Some(mut codec) = Codec::from_data(Data::new_copy(encoded)) else {
        return false;
    };
    let dimensions = codec.dimensions();
    let Some(len) = (dimensions.width as usize)
        .checked_mul(dimensions.height as usize)
        .and_then(|count| count.checked_mul(4))
        .filter(|len| *len > 0 && *len <= MAX_IMAGE_BYTES)
    else {
        return false;
    };
    let info = ImageInfo::new(dimensions, ColorType::RGBA8888, AlphaType::Unpremul, None);
    let data = unsafe { ghostty_alloc(allocator, len) };
    if data.is_null() {
        return false;
    }
    unsafe {
        // Initialize the foreign allocation before borrowing it as Rust bytes.
        data.write_bytes(0, len);
        let pixels = slice::from_raw_parts_mut(data, len);
        let result =
            codec.get_pixels_with_options(&info, pixels, dimensions.width as usize * 4, None);
        if result != skia_safe::codec::Result::Success {
            ghostty_free(allocator, data, len);
            return false;
        }
        output.write(GhosttySysImage {
            width: dimensions.width as u32,
            height: dimensions.height as u32,
            data,
            data_len: len,
        });
    }
    true
}
