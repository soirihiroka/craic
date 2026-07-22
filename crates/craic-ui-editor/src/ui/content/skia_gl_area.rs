use super::skia_canvas;
use gtk::prelude::*;
use skia_safe::gpu::{
    SurfaceOrigin, backend_render_targets, direct_contexts, gl as skia_gl, surfaces,
};
use skia_safe::{ColorSpace, ColorType};
use std::cell::RefCell;
use std::ffi::{CString, c_char, c_void};
use std::rc::Rc;
use std::sync::Once;

#[link(name = "EGL")]
unsafe extern "C" {
    fn eglGetProcAddress(name: *const c_char) -> Option<unsafe extern "C" fn()>;
}

struct GpuState {
    context: skia_safe::gpu::DirectContext,
}

pub fn new_area() -> gtk::GLArea {
    let area = gtk::GLArea::builder()
        .auto_render(false)
        .has_depth_buffer(false)
        .has_stencil_buffer(true)
        .focusable(true)
        .hexpand(true)
        .vexpand(true)
        .build();
    area.set_required_version(3, 0);
    area
}

pub fn install<F>(area: &gtk::GLArea, draw: F)
where
    F: Fn(&gtk::GLArea, &skia_canvas::Context<'_>, i32, i32) + 'static,
{
    let gpu = Rc::new(RefCell::new(None::<GpuState>));

    area.connect_realize({
        let gpu = gpu.clone();
        move |area| {
            if area
                .display()
                .downcast_ref::<gdk4_wayland::WaylandDisplay>()
                .is_none()
            {
                let error = gtk::glib::Error::new(
                    gtk::glib::FileError::Failed,
                    "The Skia code view requires a Wayland display",
                );
                log::error!("skia_gl_area rejected non-Wayland display");
                area.set_error(Some(&error));
                return;
            }

            area.make_current();
            if let Some(error) = area.error() {
                log::error!("skia_gl_area failed to create GL context: {error}");
                return;
            }

            static LOAD_GL: Once = Once::new();
            LOAD_GL.call_once(|| gl::load_with(load_egl_gl_symbol));
            let Some(interface) = skia_gl::Interface::new_load_with(load_egl_gl_symbol) else {
                let error = gtk::glib::Error::new(
                    gtk::glib::FileError::Failed,
                    "Skia could not load the current OpenGL interface",
                );
                log::error!("skia_gl_area failed to load OpenGL interface");
                area.set_error(Some(&error));
                return;
            };
            let Some(context) = direct_contexts::make_gl(interface, None) else {
                let error = gtk::glib::Error::new(
                    gtk::glib::FileError::Failed,
                    "Skia could not create a GPU rendering context",
                );
                log::error!("skia_gl_area failed to create Skia direct context");
                area.set_error(Some(&error));
                return;
            };

            log::debug!("skia_gl_area initialized Wayland OpenGL renderer");
            gpu.replace(Some(GpuState { context }));
        }
    });

    area.connect_unrealize({
        let gpu = gpu.clone();
        move |area| {
            area.make_current();
            if let Some(mut gpu) = gpu.borrow_mut().take() {
                gpu.context.release_resources_and_abandon();
                log::debug!("skia_gl_area released GPU resources before unrealize");
            }
        }
    });

    area.connect_render(move |area, _gl_context| {
        let mut gpu = gpu.borrow_mut();
        let Some(gpu) = gpu.as_mut() else {
            return gtk::glib::Propagation::Stop;
        };

        let mut viewport = [0_i32; 4];
        let mut framebuffer = 0_i32;
        let mut samples = 0_i32;
        let mut stencil_bits = 0_i32;
        unsafe {
            gl::GetIntegerv(gl::VIEWPORT, viewport.as_mut_ptr());
            gl::GetIntegerv(gl::FRAMEBUFFER_BINDING, &mut framebuffer);
            gl::GetIntegerv(gl::SAMPLES, &mut samples);
            gl::GetIntegerv(GL_STENCIL_BITS, &mut stencil_bits);
        }
        let physical_width = viewport[2].max(1);
        let physical_height = viewport[3].max(1);
        let logical_width = area.allocated_width().max(1);
        let logical_height = area.allocated_height().max(1);
        let info = skia_gl::FramebufferInfo {
            fboid: framebuffer.max(0) as u32,
            format: skia_gl::Format::RGBA8.into(),
            ..Default::default()
        };
        let target = backend_render_targets::make_gl(
            (physical_width, physical_height),
            samples.max(0) as usize,
            stencil_bits.max(0) as usize,
            info,
        );
        let Some(mut surface) = surfaces::wrap_backend_render_target(
            &mut gpu.context,
            &target,
            SurfaceOrigin::BottomLeft,
            ColorType::RGBA8888,
            ColorSpace::new_srgb(),
            None,
        ) else {
            let error = gtk::glib::Error::new(
                gtk::glib::FileError::Failed,
                "Skia could not wrap the GLArea framebuffer",
            );
            log::error!(
                "skia_gl_area failed to wrap framebuffer fbo={framebuffer} size={physical_width}x{physical_height} samples={samples} stencil={stencil_bits}"
            );
            area.set_error(Some(&error));
            return gtk::glib::Propagation::Stop;
        };

        let canvas = surface.canvas();
        canvas.clear(skia_safe::Color::TRANSPARENT);
        canvas.save();
        canvas.scale((
            physical_width as f32 / logical_width as f32,
            physical_height as f32 / logical_height as f32,
        ));
        let context = skia_canvas::Context::new(canvas);
        draw(area, &context, logical_width, logical_height);
        canvas.restore();
        gpu.context.flush_and_submit();
        gtk::glib::Propagation::Stop
    });
}

const GL_STENCIL_BITS: u32 = 0x0D57;

fn load_egl_gl_symbol(name: &str) -> *const c_void {
    let Ok(name) = CString::new(name) else {
        return std::ptr::null();
    };
    unsafe {
        eglGetProcAddress(name.as_ptr())
            .map(|symbol| symbol as *const () as *const c_void)
            .unwrap_or(std::ptr::null())
    }
}
