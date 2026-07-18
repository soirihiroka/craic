// Portions of this renderer are derived from Alacritty's OpenGL renderer.
// Alacritty is licensed under Apache-2.0.

use alacritty_terminal::index::Point;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{RenderableContent, Term};
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};
use epoxy as gl;
use gl::types::{GLchar, GLenum, GLint, GLsizei, GLuint};
use gtk::pango::{self, FontDescription};
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_void};
use std::mem::size_of;
use std::ptr;

use super::TerminalEventProxy;

const ATLAS_SIZE: i32 = 1024;
const DEFAULT_FOREGROUND: Rgb = Rgb {
    r: 212,
    g: 212,
    b: 212,
};
const DEFAULT_BACKGROUND: Rgb = Rgb {
    r: 30,
    g: 30,
    b: 30,
};
const CURSOR_COLOR: Rgb = Rgb {
    r: 174,
    g: 175,
    b: 173,
};
const SELECTION_FOREGROUND: Rgb = Rgb {
    r: 255,
    g: 255,
    b: 255,
};
const SELECTION_BACKGROUND: Rgb = Rgb {
    r: 38,
    g: 79,
    b: 120,
};
const DIM_FACTOR: f32 = 0.66;
const ANSI_COLORS: [Rgb; 16] = [
    Rgb { r: 0, g: 0, b: 0 },
    Rgb {
        r: 205,
        g: 49,
        b: 49,
    },
    Rgb {
        r: 13,
        g: 188,
        b: 121,
    },
    Rgb {
        r: 229,
        g: 229,
        b: 16,
    },
    Rgb {
        r: 36,
        g: 114,
        b: 200,
    },
    Rgb {
        r: 188,
        g: 63,
        b: 188,
    },
    Rgb {
        r: 17,
        g: 168,
        b: 205,
    },
    Rgb {
        r: 229,
        g: 229,
        b: 229,
    },
    Rgb {
        r: 102,
        g: 102,
        b: 102,
    },
    Rgb {
        r: 241,
        g: 76,
        b: 76,
    },
    Rgb {
        r: 35,
        g: 209,
        b: 139,
    },
    Rgb {
        r: 245,
        g: 245,
        b: 67,
    },
    Rgb {
        r: 59,
        g: 142,
        b: 234,
    },
    Rgb {
        r: 214,
        g: 112,
        b: 214,
    },
    Rgb {
        r: 41,
        g: 184,
        b: 219,
    },
    Rgb {
        r: 255,
        g: 255,
        b: 255,
    },
];

const VERTEX_SHADER: &str = r#"#version 150
in vec2 position;
in vec2 textureCoordinates;
in vec4 color;
in float multicolor;

uniform vec2 viewport;

out vec2 fragmentTextureCoordinates;
out vec4 fragmentColor;
out float fragmentMulticolor;

void main() {
    vec2 clip = vec2(
        position.x / viewport.x * 2.0 - 1.0,
        1.0 - position.y / viewport.y * 2.0
    );
    gl_Position = vec4(clip, 0.0, 1.0);
    fragmentTextureCoordinates = textureCoordinates;
    fragmentColor = color;
    fragmentMulticolor = multicolor;
}
"#;

const FRAGMENT_SHADER: &str = r#"#version 150
in vec2 fragmentTextureCoordinates;
in vec4 fragmentColor;
in float fragmentMulticolor;

uniform sampler2D glyphTexture;

out vec4 outputColor;

void main() {
    vec4 sampleColor = texture(glyphTexture, fragmentTextureCoordinates);
    if (fragmentMulticolor > 0.5) {
        outputColor = sampleColor;
    } else {
        outputColor = vec4(fragmentColor.rgb * sampleColor.a, fragmentColor.a * sampleColor.a);
    }
}
"#;

const GLES_VERTEX_SHADER: &str = r#"#version 300 es
precision highp float;

in vec2 position;
in vec2 textureCoordinates;
in vec4 color;
in float multicolor;

uniform vec2 viewport;

out vec2 fragmentTextureCoordinates;
out vec4 fragmentColor;
out float fragmentMulticolor;

void main() {
    vec2 clip = vec2(
        position.x / viewport.x * 2.0 - 1.0,
        1.0 - position.y / viewport.y * 2.0
    );
    gl_Position = vec4(clip, 0.0, 1.0);
    fragmentTextureCoordinates = textureCoordinates;
    fragmentColor = color;
    fragmentMulticolor = multicolor;
}
"#;

const GLES_FRAGMENT_SHADER: &str = r#"#version 300 es
precision mediump float;

in vec2 fragmentTextureCoordinates;
in vec4 fragmentColor;
in float fragmentMulticolor;

uniform sampler2D glyphTexture;

layout(location = 0) out vec4 outputColor;

void main() {
    vec4 sampleColor = texture(glyphTexture, fragmentTextureCoordinates);
    if (fragmentMulticolor > 0.5) {
        outputColor = sampleColor;
    } else {
        outputColor = vec4(fragmentColor.rgb * sampleColor.a, fragmentColor.a * sampleColor.a);
    }
}
"#;

#[derive(Clone, Copy, Debug)]
pub struct TerminalSize {
    pub width: u32,
    pub height: u32,
    pub cell_width: f32,
    pub cell_height: f32,
}

impl alacritty_terminal::grid::Dimensions for TerminalSize {
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }

    fn screen_lines(&self) -> usize {
        ((self.height as f32 / self.cell_height).floor() as usize).max(1)
    }

    fn columns(&self) -> usize {
        ((self.width as f32 / self.cell_width).floor() as usize).max(2)
    }
}

impl From<TerminalSize> for alacritty_terminal::event::WindowSize {
    fn from(size: TerminalSize) -> Self {
        use alacritty_terminal::grid::Dimensions;

        Self {
            num_lines: size.screen_lines().min(u16::MAX as usize) as u16,
            num_cols: size.columns().min(u16::MAX as usize) as u16,
            cell_width: size.cell_width.ceil().min(u16::MAX as f32) as u16,
            cell_height: size.cell_height.ceil().min(u16::MAX as f32) as u16,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Vertex {
    position: [f32; 2],
    uv: [f32; 2],
    color: [u8; 4],
    multicolor: f32,
}

#[derive(Clone, Copy)]
struct Glyph {
    texture: GLuint,
    multicolor: bool,
    top: i32,
    left: i32,
    width: i32,
    height: i32,
    uv_left: f32,
    uv_top: f32,
    uv_width: f32,
    uv_height: f32,
}

struct RasterizedGlyph {
    pixels: Vec<u8>,
    top: i32,
    left: i32,
    width: i32,
    height: i32,
}

struct Atlas {
    texture: GLuint,
    row_x: i32,
    row_y: i32,
    row_height: i32,
}

impl Atlas {
    fn new() -> Self {
        let mut texture = 0;
        unsafe {
            gl::PixelStorei(gl::UNPACK_ALIGNMENT, 1);
            gl::GenTextures(1, &mut texture);
            gl::BindTexture(gl::TEXTURE_2D, texture);
            gl::TexImage2D(
                gl::TEXTURE_2D,
                0,
                gl::RGBA as GLint,
                ATLAS_SIZE,
                ATLAS_SIZE,
                0,
                gl::RGBA,
                gl::UNSIGNED_BYTE,
                ptr::null(),
            );
            gl::TexParameteri(
                gl::TEXTURE_2D,
                gl::TEXTURE_WRAP_S,
                gl::CLAMP_TO_EDGE as GLint,
            );
            gl::TexParameteri(
                gl::TEXTURE_2D,
                gl::TEXTURE_WRAP_T,
                gl::CLAMP_TO_EDGE as GLint,
            );
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::LINEAR as GLint);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::LINEAR as GLint);
            let white = [255u8; 4];
            gl::TexSubImage2D(
                gl::TEXTURE_2D,
                0,
                0,
                0,
                1,
                1,
                gl::RGBA,
                gl::UNSIGNED_BYTE,
                white.as_ptr().cast(),
            );
            gl::BindTexture(gl::TEXTURE_2D, 0);
        }
        Self {
            texture,
            row_x: 1,
            row_y: 0,
            row_height: 1,
        }
    }

    fn insert(&mut self, glyph: &RasterizedGlyph) -> Option<Glyph> {
        if glyph.width <= 0 || glyph.height <= 0 {
            return Some(Glyph {
                texture: self.texture,
                multicolor: false,
                top: glyph.top,
                left: glyph.left,
                width: 0,
                height: 0,
                uv_left: 0.0,
                uv_top: 0.0,
                uv_width: 0.0,
                uv_height: 0.0,
            });
        }
        if glyph.width > ATLAS_SIZE || glyph.height > ATLAS_SIZE {
            return None;
        }
        if self.row_x + glyph.width > ATLAS_SIZE {
            self.row_x = 0;
            self.row_y += self.row_height;
            self.row_height = 0;
        }
        if self.row_y + glyph.height > ATLAS_SIZE {
            return None;
        }

        let x = self.row_x;
        let y = self.row_y;
        unsafe {
            gl::BindTexture(gl::TEXTURE_2D, self.texture);
            gl::TexSubImage2D(
                gl::TEXTURE_2D,
                0,
                x,
                y,
                glyph.width,
                glyph.height,
                gl::RGBA,
                gl::UNSIGNED_BYTE,
                glyph.pixels.as_ptr().cast(),
            );
            gl::BindTexture(gl::TEXTURE_2D, 0);
        }
        self.row_x += glyph.width;
        self.row_height = self.row_height.max(glyph.height);

        Some(Glyph {
            texture: self.texture,
            multicolor: false,
            top: glyph.top,
            left: glyph.left,
            width: glyph.width,
            height: glyph.height,
            uv_left: x as f32 / ATLAS_SIZE as f32,
            uv_top: y as f32 / ATLAS_SIZE as f32,
            uv_width: glyph.width as f32 / ATLAS_SIZE as f32,
            uv_height: glyph.height as f32 / ATLAS_SIZE as f32,
        })
    }
}

impl Drop for Atlas {
    fn drop(&mut self) {
        unsafe { gl::DeleteTextures(1, &self.texture) };
    }
}

struct FontCache {
    fonts: [FontDescription; 4],
    cell_width: f32,
    cell_height: f32,
    glyphs: HashMap<(char, usize), Glyph>,
    atlases: Vec<Atlas>,
}

impl FontCache {
    fn new(font_size: f64, scale: f32) -> Result<Self, String> {
        let fonts = std::array::from_fn(|index| {
            let mut font = FontDescription::new();
            font.set_family("monospace");
            font.set_absolute_size(font_size * scale as f64 * pango::SCALE as f64);
            if matches!(index, 1 | 3) {
                font.set_weight(pango::Weight::Bold);
            }
            if matches!(index, 2 | 3) {
                font.set_style(pango::Style::Italic);
            }
            font
        });
        let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 1, 1)
            .map_err(|err| err.to_string())?;
        let context = cairo::Context::new(&surface).map_err(|err| err.to_string())?;
        let layout = pangocairo::functions::create_layout(&context);
        layout.set_font_description(Some(&fonts[0]));
        layout.set_single_paragraph_mode(true);
        layout.set_text("M");
        let (_, logical) = layout.pixel_extents();
        let cell_width = logical.width().max(1) as f32;
        let cell_height = logical.height().max(1) as f32;

        Ok(Self {
            fonts,
            cell_width,
            cell_height,
            glyphs: HashMap::new(),
            atlases: vec![Atlas::new()],
        })
    }

    fn cell_size(&self) -> (f32, f32) {
        (self.cell_width, self.cell_height)
    }

    fn glyph(&mut self, character: char, flags: Flags) -> Glyph {
        let index = match flags & Flags::BOLD_ITALIC {
            Flags::BOLD_ITALIC => 3,
            Flags::BOLD => 1,
            Flags::ITALIC => 2,
            _ => 0,
        };
        let key = (character, index);
        if let Some(glyph) = self.glyphs.get(&key) {
            return *glyph;
        }

        let rasterized = self.rasterize(character, index).unwrap_or_else(|err| {
            log::warn!("terminal glyph rasterization failed character={character:?}: {err}");
            RasterizedGlyph {
                pixels: Vec::new(),
                top: 0,
                left: 0,
                width: 0,
                height: 0,
            }
        });
        let mut glyph = None;
        for atlas in &mut self.atlases {
            if let Some(inserted) = atlas.insert(&rasterized) {
                glyph = Some(inserted);
                break;
            }
        }
        let glyph = glyph.unwrap_or_else(|| {
            let mut atlas = Atlas::new();
            let glyph = atlas.insert(&rasterized).unwrap_or(Glyph {
                texture: atlas.texture,
                multicolor: false,
                top: 0,
                left: 0,
                width: 0,
                height: 0,
                uv_left: 0.0,
                uv_top: 0.0,
                uv_width: 0.0,
                uv_height: 0.0,
            });
            self.atlases.push(atlas);
            glyph
        });
        self.glyphs.insert(key, glyph);
        glyph
    }

    fn rasterize(&self, character: char, style: usize) -> Result<RasterizedGlyph, String> {
        let scratch = cairo::ImageSurface::create(cairo::Format::ARgb32, 1, 1)
            .map_err(|err| err.to_string())?;
        let scratch_context = cairo::Context::new(&scratch).map_err(|err| err.to_string())?;
        let layout = pangocairo::functions::create_layout(&scratch_context);
        layout.set_font_description(Some(&self.fonts[style]));
        layout.set_single_paragraph_mode(true);
        layout.set_text(&character.to_string());
        let (ink, _) = layout.pixel_extents();
        let width = ink.width().max(0);
        let height = ink.height().max(0);
        if width == 0 || height == 0 {
            return Ok(RasterizedGlyph {
                pixels: Vec::new(),
                top: 0,
                left: 0,
                width: 0,
                height: 0,
            });
        }

        let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height)
            .map_err(|err| err.to_string())?;
        {
            let context = cairo::Context::new(&surface).map_err(|err| err.to_string())?;
            context.set_source_rgba(1.0, 1.0, 1.0, 1.0);
            context.move_to((-ink.x()) as f64, (-ink.y()) as f64);
            pangocairo::functions::show_layout(&context, &layout);
        }
        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().map_err(|err| err.to_string())?;
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
        for row in 0..height as usize {
            for column in 0..width as usize {
                let pixel = row * stride + column * 4;
                #[cfg(target_endian = "little")]
                let alpha = data[pixel + 3];
                #[cfg(target_endian = "big")]
                let alpha = data[pixel];
                pixels.extend_from_slice(&[255, 255, 255, alpha]);
            }
        }

        Ok(RasterizedGlyph {
            pixels,
            top: self.cell_height as i32 - ink.y(),
            left: ink.x(),
            width,
            height,
        })
    }
}

pub struct GlRenderer {
    program: GLuint,
    vao: GLuint,
    vbo: GLuint,
    font: FontCache,
    size: TerminalSize,
    scale: f32,
}

impl GlRenderer {
    pub fn new(
        font_size: f64,
        scale: f32,
        width: u32,
        height: u32,
        uses_es: bool,
    ) -> Result<Self, String> {
        load_epoxy();
        let gl_version = gl_string(gl::VERSION).unwrap_or_else(|| "unknown".to_string());
        let gl_renderer = gl_string(gl::RENDERER).unwrap_or_else(|| "unknown".to_string());
        log::info!(
            "alacritty GL renderer initializing version={gl_version} gpu={gl_renderer} api={}",
            if uses_es { "gles" } else { "gl" }
        );

        let vertex = compile_shader(
            gl::VERTEX_SHADER,
            if uses_es { GLES_VERTEX_SHADER } else { VERTEX_SHADER },
        )?;
        let fragment = compile_shader(
            gl::FRAGMENT_SHADER,
            if uses_es {
                GLES_FRAGMENT_SHADER
            } else {
                FRAGMENT_SHADER
            },
        )?;
        let program = link_program(vertex, fragment, uses_es)?;
        unsafe {
            gl::DeleteShader(vertex);
            gl::DeleteShader(fragment);
        }

        let font = FontCache::new(font_size, scale)?;
        let (cell_width, cell_height) = font.cell_size();
        let size = TerminalSize {
            width: width.max(cell_width.ceil() as u32),
            height: height.max(cell_height.ceil() as u32),
            cell_width,
            cell_height,
        };

        let mut vao = 0;
        let mut vbo = 0;
        unsafe {
            gl::GenVertexArrays(1, &mut vao);
            gl::GenBuffers(1, &mut vbo);
            gl::BindVertexArray(vao);
            gl::BindBuffer(gl::ARRAY_BUFFER, vbo);
            let stride = size_of::<Vertex>() as GLsizei;
            gl::EnableVertexAttribArray(0);
            gl::VertexAttribPointer(0, 2, gl::FLOAT, gl::FALSE, stride, ptr::null());
            gl::EnableVertexAttribArray(1);
            gl::VertexAttribPointer(
                1,
                2,
                gl::FLOAT,
                gl::FALSE,
                stride,
                (2 * size_of::<f32>()) as *const c_void,
            );
            gl::EnableVertexAttribArray(2);
            gl::VertexAttribPointer(
                2,
                4,
                gl::UNSIGNED_BYTE,
                gl::TRUE,
                stride,
                (4 * size_of::<f32>()) as *const c_void,
            );
            gl::EnableVertexAttribArray(3);
            gl::VertexAttribPointer(
                3,
                1,
                gl::FLOAT,
                gl::FALSE,
                stride,
                (4 * size_of::<f32>() + 4 * size_of::<u8>()) as *const c_void,
            );
            gl::BindBuffer(gl::ARRAY_BUFFER, 0);
            gl::BindVertexArray(0);
            gl::UseProgram(program);
            gl::Uniform1i(gl::GetUniformLocation(program, c"glyphTexture".as_ptr()), 0);
            gl::UseProgram(0);
        }

        Ok(Self {
            program,
            vao,
            vbo,
            font,
            size,
            scale,
        })
    }

    pub fn size(&self) -> TerminalSize {
        self.size
    }

    pub fn resize(&mut self, width: u32, height: u32) -> bool {
        let width = width.max(self.size.cell_width.ceil() as u32);
        let height = height.max(self.size.cell_height.ceil() as u32);
        if self.size.width == width && self.size.height == height {
            return false;
        }
        self.size.width = width;
        self.size.height = height;
        true
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    pub fn draw(&mut self, term: &Term<TerminalEventProxy>, focused: bool) {
        let background = color_for_index(term.colors(), NamedColor::Background as usize);
        unsafe {
            gl::Viewport(0, 0, self.size.width as i32, self.size.height as i32);
            gl::ClearColor(
                background.r as f32 / 255.0,
                background.g as f32 / 255.0,
                background.b as f32 / 255.0,
                1.0,
            );
            gl::Clear(gl::COLOR_BUFFER_BIT);
            gl::Enable(gl::BLEND);
            gl::BlendFunc(gl::ONE, gl::ONE_MINUS_SRC_ALPHA);
        }

        let mut batches: HashMap<GLuint, Vec<Vertex>> = HashMap::new();
        let mut backgrounds = Vec::new();
        let content = term.renderable_content();
        self.collect_cells(content, focused, &mut backgrounds, &mut batches);

        let white_uv = 0.5 / ATLAS_SIZE as f32;
        self.draw_vertices(
            self.font.atlases[0].texture,
            backgrounds
                .into_iter()
                .flat_map(|(point, color)| {
                    quad(
                        point.column.0 as f32 * self.size.cell_width,
                        point.line as f32 * self.size.cell_height,
                        self.size.cell_width,
                        self.size.cell_height,
                        [white_uv, white_uv, 0.0, 0.0],
                        color,
                        false,
                    )
                })
                .collect(),
        );
        for (texture, vertices) in batches {
            self.draw_vertices(texture, vertices);
        }

        unsafe {
            gl::BindTexture(gl::TEXTURE_2D, 0);
            gl::BindBuffer(gl::ARRAY_BUFFER, 0);
            gl::BindVertexArray(0);
            gl::UseProgram(0);
        }
    }

    fn collect_cells(
        &mut self,
        content: RenderableContent<'_>,
        focused: bool,
        backgrounds: &mut Vec<(Point<usize>, Rgb)>,
        batches: &mut HashMap<GLuint, Vec<Vertex>>,
    ) {
        let RenderableContent {
            display_iter,
            selection,
            cursor,
            display_offset,
            colors,
            ..
        } = content;
        let cursor_point =
            alacritty_terminal::term::point_to_viewport(display_offset, cursor.point);
        for indexed in display_iter {
            let Some(viewport_point) =
                alacritty_terminal::term::point_to_viewport(display_offset, indexed.point)
            else {
                continue;
            };
            if indexed.cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let mut fg = foreground_color(colors, indexed.cell.fg, indexed.cell.flags);
            let mut bg = background_color(colors, indexed.cell.bg);
            if indexed.cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if selection.is_some_and(|selection| selection.contains(indexed.point)) {
                fg = SELECTION_FOREGROUND;
                bg = SELECTION_BACKGROUND;
            }
            if focused
                && cursor.shape == CursorShape::Block
                && cursor_point == Some(viewport_point)
            {
                fg = bg;
                bg = color_for_index(colors, NamedColor::Cursor as usize);
            }

            backgrounds.push((viewport_point, bg));
            if indexed.cell.flags.contains(Flags::HIDDEN) || indexed.cell.c == ' ' {
                continue;
            }
            self.push_glyph(
                viewport_point,
                indexed.cell.c,
                indexed.cell.flags,
                fg,
                batches,
            );
            if let Some(zerowidth) = indexed.cell.zerowidth() {
                for character in zerowidth {
                    self.push_glyph(viewport_point, *character, indexed.cell.flags, fg, batches);
                }
            }
        }
    }

    fn push_glyph(
        &mut self,
        point: Point<usize>,
        character: char,
        flags: Flags,
        color: Rgb,
        batches: &mut HashMap<GLuint, Vec<Vertex>>,
    ) {
        let glyph = self.font.glyph(character, flags);
        if glyph.width == 0 || glyph.height == 0 {
            return;
        }
        let x = point.column.0 as f32 * self.size.cell_width + glyph.left as f32;
        let y = (point.line + 1) as f32 * self.size.cell_height - glyph.top as f32;
        let uv = [glyph.uv_left, glyph.uv_top, glyph.uv_width, glyph.uv_height];
        batches.entry(glyph.texture).or_default().extend(quad(
            x,
            y,
            glyph.width as f32,
            glyph.height as f32,
            uv,
            color,
            glyph.multicolor,
        ));
    }

    fn draw_vertices(&self, texture: GLuint, vertices: Vec<Vertex>) {
        if vertices.is_empty() {
            return;
        }
        unsafe {
            gl::UseProgram(self.program);
            gl::Uniform2f(
                gl::GetUniformLocation(self.program, c"viewport".as_ptr()),
                self.size.width as f32,
                self.size.height as f32,
            );
            gl::ActiveTexture(gl::TEXTURE0);
            gl::BindTexture(gl::TEXTURE_2D, texture);
            gl::BindVertexArray(self.vao);
            gl::BindBuffer(gl::ARRAY_BUFFER, self.vbo);
            gl::BufferData(
                gl::ARRAY_BUFFER,
                std::mem::size_of_val(vertices.as_slice()) as isize,
                vertices.as_ptr().cast(),
                gl::STREAM_DRAW,
            );
            gl::DrawArrays(gl::TRIANGLES, 0, vertices.len() as GLsizei);
        }
    }
}

impl Drop for GlRenderer {
    fn drop(&mut self) {
        unsafe {
            gl::DeleteBuffers(1, &self.vbo);
            gl::DeleteVertexArrays(1, &self.vao);
            gl::DeleteProgram(self.program);
        }
    }
}

fn foreground_color(
    colors: &alacritty_terminal::term::color::Colors,
    color: Color,
    flags: Flags,
) -> Rgb {
    match color {
        Color::Spec(rgb) if flags.contains(Flags::DIM) => rgb * DIM_FACTOR,
        Color::Spec(rgb) => rgb,
        Color::Indexed(index) => {
            let index = if flags & Flags::DIM_BOLD == Flags::BOLD && index <= 7 {
                index + 8
            } else {
                index
            };
            color_for_index(colors, index as usize)
        }
        Color::Named(named) => {
            let named = match flags & Flags::DIM_BOLD {
                Flags::DIM_BOLD if named == NamedColor::Foreground => NamedColor::DimForeground,
                Flags::BOLD => named.to_bright(),
                Flags::DIM => named.to_dim(),
                _ => named,
            };
            color_for_index(colors, named as usize)
        }
    }
}

fn background_color(
    colors: &alacritty_terminal::term::color::Colors,
    color: Color,
) -> Rgb {
    match color {
        Color::Spec(rgb) => rgb,
        Color::Indexed(index) => color_for_index(colors, index as usize),
        Color::Named(named) => color_for_index(colors, named as usize),
    }
}

pub(super) fn color_for_index(
    colors: &alacritty_terminal::term::color::Colors,
    index: usize,
) -> Rgb {
    if index < alacritty_terminal::term::color::COUNT
        && let Some(color) = colors[index]
    {
        return color;
    }
    if index < ANSI_COLORS.len() {
        return ANSI_COLORS[index];
    }
    if index < 256 {
        return indexed_color(index as u8);
    }
    if index == NamedColor::Background as usize {
        return DEFAULT_BACKGROUND;
    }
    if index == NamedColor::Cursor as usize {
        return CURSOR_COLOR;
    }
    if index == NamedColor::DimForeground as usize {
        return DEFAULT_FOREGROUND * DIM_FACTOR;
    }
    if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&index) {
        return ANSI_COLORS[index - NamedColor::DimBlack as usize] * DIM_FACTOR;
    }
    DEFAULT_FOREGROUND
}

fn indexed_color(index: u8) -> Rgb {
    if index < 16 {
        return ANSI_COLORS[index as usize];
    }
    if index < 232 {
        let index = index - 16;
        let component = |value: u8| if value == 0 { 0 } else { 55 + value * 40 };
        return Rgb {
            r: component(index / 36),
            g: component((index / 6) % 6),
            b: component(index % 6),
        };
    }
    let value = 8 + (index - 232) * 10;
    Rgb {
        r: value,
        g: value,
        b: value,
    }
}

fn quad(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    uv: [f32; 4],
    color: Rgb,
    multicolor: bool,
) -> [Vertex; 6] {
    let [u, v, uw, vh] = uv;
    let color = [color.r, color.g, color.b, 255];
    let multicolor = if multicolor { 1.0 } else { 0.0 };
    let top_left = Vertex {
        position: [x, y],
        uv: [u, v],
        color,
        multicolor,
    };
    let top_right = Vertex {
        position: [x + width, y],
        uv: [u + uw, v],
        color,
        multicolor,
    };
    let bottom_left = Vertex {
        position: [x, y + height],
        uv: [u, v + vh],
        color,
        multicolor,
    };
    let bottom_right = Vertex {
        position: [x + width, y + height],
        uv: [u + uw, v + vh],
        color,
        multicolor,
    };
    [
        top_left,
        bottom_left,
        top_right,
        top_right,
        bottom_left,
        bottom_right,
    ]
}

fn load_epoxy() {
    epoxy::load_with(|symbol| {
        let Ok(symbol) = CString::new(symbol) else {
            return ptr::null();
        };
        unsafe { libc::dlsym(libc::RTLD_DEFAULT, symbol.as_ptr()) }
    });
}

fn gl_string(name: GLenum) -> Option<String> {
    let value = unsafe { gl::GetString(name) };
    if value.is_null() {
        return None;
    }
    Some(
        unsafe { CStr::from_ptr(value.cast()) }
            .to_string_lossy()
            .into_owned(),
    )
}

fn compile_shader(kind: GLenum, source: &str) -> Result<GLuint, String> {
    let shader = unsafe { gl::CreateShader(kind) };
    let source = CString::new(source).map_err(|err| err.to_string())?;
    unsafe {
        gl::ShaderSource(shader, 1, &source.as_ptr(), ptr::null());
        gl::CompileShader(shader);
    }
    let mut success = 0;
    unsafe { gl::GetShaderiv(shader, gl::COMPILE_STATUS, &mut success) };
    if success == gl::TRUE as GLint {
        return Ok(shader);
    }
    let message = shader_log(shader);
    unsafe { gl::DeleteShader(shader) };
    Err(message)
}

fn link_program(vertex: GLuint, fragment: GLuint, uses_es: bool) -> Result<GLuint, String> {
    let program = unsafe { gl::CreateProgram() };
    unsafe {
        gl::AttachShader(program, vertex);
        gl::AttachShader(program, fragment);
        gl::BindAttribLocation(program, 0, c"position".as_ptr());
        gl::BindAttribLocation(program, 1, c"textureCoordinates".as_ptr());
        gl::BindAttribLocation(program, 2, c"color".as_ptr());
        gl::BindAttribLocation(program, 3, c"multicolor".as_ptr());
        if !uses_es {
            gl::BindFragDataLocation(program, 0, c"outputColor".as_ptr());
        }
        gl::LinkProgram(program);
    }
    let mut success = 0;
    unsafe { gl::GetProgramiv(program, gl::LINK_STATUS, &mut success) };
    if success == gl::TRUE as GLint {
        return Ok(program);
    }
    let mut length = 0;
    unsafe { gl::GetProgramiv(program, gl::INFO_LOG_LENGTH, &mut length) };
    let mut buffer = vec![0u8; length.max(1) as usize];
    unsafe {
        gl::GetProgramInfoLog(
            program,
            length,
            ptr::null_mut(),
            buffer.as_mut_ptr().cast::<GLchar>(),
        );
        gl::DeleteProgram(program);
    }
    Err(String::from_utf8_lossy(&buffer)
        .trim_end_matches('\0')
        .to_string())
}

fn shader_log(shader: GLuint) -> String {
    let mut length = 0;
    unsafe { gl::GetShaderiv(shader, gl::INFO_LOG_LENGTH, &mut length) };
    let mut buffer = vec![0u8; length.max(1) as usize];
    unsafe {
        gl::GetShaderInfoLog(
            shader,
            length,
            ptr::null_mut(),
            buffer.as_mut_ptr().cast::<GLchar>(),
        );
    }
    String::from_utf8_lossy(&buffer)
        .trim_end_matches('\0')
        .to_string()
}
