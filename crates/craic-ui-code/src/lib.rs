mod editor;
mod viewport;

pub use editor::{Editor, Motion, SearchOptions};
pub use viewport::paint_editor;

use gpui::{
    AtlasTile, Bounds, ContentMask, Corners, DevicePixels, FontRun, IsZero, MonochromeSprite,
    PaintedPath, PathBuilder, Pixels, PlatformAtlas, PlatformTextSystem, PolychromeSprite, Quad,
    RenderGlyphParams, Rgba, ScaledPixels, Scene, Size, SpriteAnimationId, TransformationMatrix,
    Underline, font, point, px, size,
};
use std::borrow::Cow;
use std::collections::HashMap;

#[derive(Clone, PartialEq)]
enum Paint {
    Quad(Bounds<Pixels>, Pixels, Rgba),
    Text {
        text: String,
        x: f32,
        baseline: f32,
        font_size: f32,
        color: Rgba,
        align_right: bool,
    },
    Triangle([(f32, f32); 3], Rgba),
    Underline(Bounds<Pixels>, Rgba),
}

#[derive(Default)]
pub struct EditorScene {
    pub scene: Scene,
    glyphs: HashMap<RenderGlyphParams, Option<(Bounds<DevicePixels>, AtlasTile)>>,
}

#[derive(Clone, PartialEq)]
pub struct EditorFrame {
    pub size: Size<Pixels>,
    pub scale: f32,
    commands: Vec<Paint>,
}

impl EditorFrame {
    pub fn new(width: f32, height: f32, scale: f32) -> Self {
        Self {
            size: size(px(width), px(height)),
            scale,
            commands: Vec::new(),
        }
    }

    pub fn device_size(&self) -> Size<DevicePixels> {
        self.size
            .scale(self.scale)
            .map(|px| DevicePixels(px.0.round().max(1.0) as i32))
    }

    pub fn quad(&mut self, x: f32, y: f32, width: f32, height: f32, radius: f32, color: Rgba) {
        if width > 0.0 && height > 0.0 {
            self.commands
                .push(Paint::Quad(bounds(x, y, width, height), px(radius), color));
        }
    }

    pub fn text(
        &mut self,
        text: &str,
        x: f32,
        baseline: f32,
        color: Rgba,
        font_size: f32,
        align_right: bool,
    ) {
        if !text.is_empty() {
            self.commands.push(Paint::Text {
                text: text.replace('\t', "    "),
                x,
                baseline,
                font_size,
                color,
                align_right,
            });
        }
    }

    pub fn underline(&mut self, x: f32, y: f32, width: f32, color: Rgba) {
        if width > 0.0 {
            self.commands
                .push(Paint::Underline(bounds(x, y, width, 4.0), color));
        }
    }

    pub fn build_scene(
        &self,
        target: &mut EditorScene,
        text_system: &dyn PlatformTextSystem,
        atlas: &dyn PlatformAtlas,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let EditorScene { scene, glyphs } = target;
        scene.clear();
        let scale = self.scale;
        let viewport = ContentMask {
            bounds: Bounds::new(point(px(0.0), px(0.0)), self.size),
        };
        let mask = viewport.scale(scale);
        let font_id = text_system.font_id(&font(EDITOR_FONT_FAMILY))?;
        for command in &self.commands {
            match command {
                Paint::Quad(bounds, radius, color) => scene.insert_primitive(Quad {
                    order: 0,
                    border_style: Default::default(),
                    bounds: bounds.scale(scale),
                    content_mask: mask,
                    background: (*color).into(),
                    corner_radii: Corners::all(radius.scale(scale)),
                    border_color: Default::default(),
                    border_widths: Default::default(),
                }),
                Paint::Underline(bounds, color) => scene.insert_primitive(Underline {
                    order: 0,
                    pad: 0,
                    bounds: bounds.scale(scale),
                    content_mask: mask,
                    color: (*color).into(),
                    thickness: px(1.2).scale(scale),
                    wavy: true.into(),
                }),
                Paint::Triangle(vertices, color) => {
                    let mut path = PathBuilder::fill();
                    let [a, b, c] = vertices.map(|(x, y)| point(px(x), px(y)));
                    path.move_to(a);
                    path.line_to(b);
                    path.line_to(c);
                    path.close();
                    if let Ok(path) = path.build() {
                        scene.insert_primitive(PaintedPath::new(
                            path,
                            scale,
                            viewport,
                            (*color).into(),
                        ));
                    }
                }
                Paint::Text {
                    text,
                    x,
                    baseline,
                    font_size,
                    color,
                    align_right,
                } => {
                    let font_size = px(*font_size);
                    let line = text_system.layout_line(
                        text,
                        font_size,
                        &[FontRun {
                            len: text.len(),
                            font_id,
                        }],
                    );
                    let x = if *align_right {
                        *x - f32::from(line.width)
                    } else {
                        *x
                    };
                    for run in line.runs {
                        for glyph in run.glyphs {
                            let origin =
                                (point(px(x), px(*baseline)) + glyph.position).scale(scale);
                            // Offscreen glyphs need no atlas entry. Leave space for glyph overhang.
                            let padding = font_size.scale(scale).0 * 2.0;
                            if origin.x.0 < -padding
                                || origin.x.0 > mask.bounds.size.width.0 + padding
                            {
                                continue;
                            }
                            let params = RenderGlyphParams {
                                font_id: run.font_id,
                                glyph_id: glyph.id,
                                font_size,
                                subpixel_variant: Default::default(),
                                scale_factor: scale,
                                is_emoji: glyph.is_emoji,
                                subpixel_rendering: false,
                                dilation: if glyph.is_emoji {
                                    0
                                } else {
                                    text_system.glyph_dilation_for_color((*color).into())
                                },
                            };
                            let raster = match glyphs.entry(params.clone()) {
                                std::collections::hash_map::Entry::Occupied(entry) => *entry.get(),
                                std::collections::hash_map::Entry::Vacant(entry) => {
                                    let mut raster = None;
                                    // Unsupported glyphs are skipped without discarding the frame.
                                    if let Ok(bounds) = text_system.glyph_raster_bounds(&params)
                                        && !bounds.is_zero()
                                    {
                                        let tile = atlas.get_or_insert_with(
                                            params.clone().into(),
                                            &mut || {
                                                let (size, bytes) =
                                                    text_system.rasterize_glyph(&params, bounds)?;
                                                Ok(Some((size, Cow::Owned(bytes))))
                                            },
                                        )?;
                                        raster = tile.map(|tile| (bounds, tile));
                                    }
                                    *entry.insert(raster)
                                }
                            };
                            let Some((raster_bounds, tile)) = raster else {
                                continue;
                            };
                            let bounds = Bounds {
                                origin: origin.map(|px| ScaledPixels(px.0.round()))
                                    + raster_bounds.origin.map(Into::into),
                                size: tile.bounds.size.map(Into::into),
                            };
                            if glyph.is_emoji {
                                scene.insert_primitive(PolychromeSprite {
                                    order: 0,
                                    pad: 0,
                                    tile,
                                    grayscale: false.into(),
                                    opacity: color.a,
                                    bounds,
                                    content_mask: mask,
                                    corner_radii: Default::default(),
                                });
                            } else {
                                scene.insert_primitive(MonochromeSprite {
                                    order: 0,
                                    animation: SpriteAnimationId::NONE,
                                    tile,
                                    bounds,
                                    content_mask: mask,
                                    color: (*color).into(),
                                    transformation: TransformationMatrix::unit(),
                                });
                            }
                        }
                    }
                }
            }
        }
        scene.finish();
        Ok(())
    }
}

fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
    Bounds::new(point(px(x), px(y)), size(px(width), px(height)))
}

pub const EDITOR_FONT_FAMILY: &str = "JetBrainsMono Nerd Font Mono";

pub fn load_editor_font(text: &dyn PlatformTextSystem) -> Result<(), Box<dyn std::error::Error>> {
    text.add_fonts(vec![Cow::Borrowed(include_bytes!(
        "../../craic-ui-core/assets/fonts/JetBrainsMono/JetBrainsMonoNerdFontMono-Regular.ttf"
    ))])?;
    Ok(())
}
