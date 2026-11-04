//! Window + GL context bring-up, carried over logic-for-logic from
//! the spike driver (`ref/driver-fixed/src/app_frame.rs`, itself the
//! inox2d `render-opengl` example's frame, BSD-2-Clause). The GL
//! config selection (alpha size 8, max samples, transparency
//! preference) and the context fallback chain (core → GLES → 2.1)
//! are kept identical so both oracle arms — and the corpus renders —
//! run on the same config.
//!
//! UNSAFE SEAM: glutin context creation, GL symbol loading and the
//! GL debug callback are `unsafe` FFI in glutin/glow. This module is
//! the only place the crate allows `unsafe`; each block is a direct
//! glutin/glow call with no pointer arithmetic of our own.
#![allow(unsafe_code)]

use std::error::Error;
use std::ffi::CString;
use std::num::NonZeroU32;

use glow::HasContext;
use glutin::config::ConfigTemplateBuilder;
use glutin::context::{ContextApi, ContextAttributesBuilder, NotCurrentContext, Version};
use glutin::display::GetGlDisplay;
use glutin::prelude::*;
use glutin::surface::SwapInterval;
use glutin_winit::{DisplayBuilder, GlWindow};
use raw_window_handle::HasRawWindowHandle;
use winit::event::{Event, WindowEvent};
use winit::event_loop::EventLoop;
use winit::window::{Window, WindowBuilder};

/// The application side of the event loop.
pub trait App {
    /// Called once per `AboutToWait` with a current context.
    fn draw(&mut self);
    /// Window resize (drawable size in physical pixels).
    fn resize(&mut self, width: u32, height: u32);
    /// Context is current: build the renderer.
    fn resume_window(&mut self, gl: glow::Context);
    /// A second GL handle on the same context, for readback.
    fn set_readback(&mut self, gl: glow::Context);
}

pub struct AppFrame {
    event_loop: EventLoop<()>,
    gl_config: glutin::config::Config,
    gl_display: glutin::display::Display,
    not_current_gl_context: Option<NotCurrentContext>,
    window: Option<Window>,
    window_builder: WindowBuilder,
}

impl AppFrame {
    pub fn init(window_builder: WindowBuilder) -> Result<Self, Box<dyn Error>> {
        let event_loop = EventLoop::new()?;
        // Spike-identical template: alpha 8; transparency is only
        // forced on macOS (EGL on X11 lacks it).
        let template = if window_builder.transparent() {
            ConfigTemplateBuilder::new()
                .with_alpha_size(8)
                .with_transparency(cfg!(target_os = "macos"))
        } else {
            ConfigTemplateBuilder::new().with_transparency(false)
        };
        let maybe_window_builder = cfg!(target_os = "windows").then_some(window_builder.clone());
        let display_builder = DisplayBuilder::new().with_window_builder(maybe_window_builder);
        let want_transparent = window_builder.transparent();
        let (window, gl_config) = display_builder.build(&event_loop, template, |configs| {
            configs
                .reduce(|accum, config| {
                    let config_transparent = config.supports_transparency().unwrap_or(false);
                    let accum_transparent = accum.supports_transparency().unwrap_or(false);
                    let transparency_check = if want_transparent {
                        config_transparent && !accum_transparent
                    } else {
                        !config_transparent && accum_transparent
                    };
                    if transparency_check || config.num_samples() > accum.num_samples() {
                        config
                    } else {
                        accum
                    }
                })
                .expect("the X server offered no GL config")
        })?;
        let raw_window_handle = window.as_ref().map(|w| w.raw_window_handle());
        let gl_display = gl_config.display();
        let core = ContextAttributesBuilder::new().build(raw_window_handle);
        let gles = ContextAttributesBuilder::new()
            .with_context_api(ContextApi::Gles(None))
            .build(raw_window_handle);
        let legacy = ContextAttributesBuilder::new()
            .with_context_api(ContextApi::OpenGl(Some(Version::new(2, 1))))
            .build(raw_window_handle);
        // SAFETY: glutin FFI; the raw window handle (if any) is the
        // live window built above.
        let context = unsafe {
            gl_display
                .create_context(&gl_config, &core)
                .or_else(|_| gl_display.create_context(&gl_config, &gles))
                .or_else(|_| gl_display.create_context(&gl_config, &legacy))?
        };
        Ok(Self {
            event_loop,
            gl_config,
            gl_display,
            not_current_gl_context: Some(context),
            window,
            window_builder,
        })
    }

    pub fn run<A: App + 'static>(mut self, mut app: A) -> Result<(), Box<dyn Error>> {
        let mut state = None;
        self.event_loop
            .run(move |event, window_target| match event {
                Event::Resumed => {
                    let window = self.window.take().unwrap_or_else(|| {
                        glutin_winit::finalize_window(
                            window_target,
                            self.window_builder.clone(),
                            &self.gl_config,
                        )
                        .expect("window finalize")
                    });
                    let attrs = window.build_surface_attributes(Default::default());
                    // SAFETY: glutin FFI on the window created above.
                    let gl_surface = unsafe {
                        self.gl_config
                            .display()
                            .create_window_surface(&self.gl_config, &attrs)
                            .expect("window surface")
                    };
                    let gl_context = self
                        .not_current_gl_context
                        .take()
                        .expect("context taken once")
                        .make_current(&gl_surface)
                        .expect("make current");
                    let mut gl = load_gl(&self.gl_display);
                    install_debug_callback(&mut gl);
                    app.set_readback(load_gl(&self.gl_display));
                    app.resume_window(gl);
                    if let Err(res) = gl_surface
                        .set_swap_interval(&gl_context, SwapInterval::Wait(NonZeroU32::MIN))
                    {
                        eprintln!("Error setting vsync: {res:?}");
                    }
                    assert!(state.replace((gl_context, gl_surface, window)).is_none());
                }
                Event::WindowEvent {
                    event: WindowEvent::Resized(size),
                    ..
                } => {
                    if let (Some(w), Some(h)) =
                        (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
                        && let Some((gl_context, gl_surface, _)) = &state
                    {
                        gl_surface.resize(gl_context, w, h);
                        app.resize(size.width, size.height);
                    }
                }
                Event::WindowEvent {
                    event: WindowEvent::CloseRequested,
                    ..
                } => std::process::exit(0),
                Event::AboutToWait => {
                    if let Some((gl_context, gl_surface, window)) = &state {
                        app.draw();
                        window.request_redraw();
                        gl_surface.swap_buffers(gl_context).expect("swap buffers");
                    }
                }
                _ => (),
            })?;
        Ok(())
    }
}

/// `(GL_VENDOR, GL_RENDERER, GL_VERSION)` of the current context.
pub fn gl_strings(gl: &glow::Context) -> [String; 3] {
    // SAFETY: plain glGetString queries on the current context.
    unsafe {
        [
            gl.get_parameter_string(glow::VENDOR),
            gl.get_parameter_string(glow::RENDERER),
            gl.get_parameter_string(glow::VERSION),
        ]
    }
}

/// Read the bound framebuffer as RGBA8, rows top-first (GL reads
/// bottom-up; flipped as the spike driver does).
pub fn read_rgba(gl: &glow::Context, width: u32, height: u32) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let mut buf = vec![0u8; w * h * 4];
    // SAFETY: `buf` is exactly w*h*4 bytes for an RGBA8 read of w×h.
    unsafe {
        gl.read_pixels(
            0,
            0,
            width as i32,
            height as i32,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(&mut buf),
        );
    }
    let row = w * 4;
    let mut flipped = vec![0u8; buf.len()];
    for y in 0..h {
        flipped[y * row..(y + 1) * row].copy_from_slice(&buf[(h - 1 - y) * row..(h - y) * row]);
    }
    flipped
}

/// Load GL function pointers from the current context.
fn load_gl(display: &glutin::display::Display) -> glow::Context {
    // SAFETY: a context is current on this thread (caller contract:
    // called only after `make_current`).
    unsafe {
        glow::Context::from_loader_function(|symbol| {
            let symbol = CString::new(symbol).expect("GL symbol names have no NUL");
            display.get_proc_address(&symbol) as *const _
        })
    }
}

/// The spike's GL_KHR_debug hook (messages go to `tracing`, which
/// the driver — like this harness — never subscribes).
fn install_debug_callback(gl: &mut glow::Context) {
    if !gl.supported_extensions().contains("GL_KHR_debug") {
        return;
    }
    // SAFETY: glow FFI with a 'static callback; the context is current.
    unsafe {
        gl.debug_message_callback(|_src, _ty, _id, sevr, msg| match sevr {
            glow::DEBUG_SEVERITY_HIGH => tracing::error!(target: "opengl", "{msg}"),
            glow::DEBUG_SEVERITY_MEDIUM => tracing::warn!(target: "opengl", "{msg}"),
            glow::DEBUG_SEVERITY_LOW => tracing::info!(target: "opengl", "{msg}"),
            _ => tracing::debug!(target: "opengl", "{msg}"),
        });
        gl.enable(glow::DEBUG_OUTPUT);
    }
}
