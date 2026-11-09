//! Native X11 capture backend.
//!
//! Uses `x11rb`'s pure-Rust, safe protocol implementation: no `unsafe` and no
//! link-time dependency on libX11. The backend covers:
//!
//! - root-window capture at the full framebuffer resolution;
//! - top-level window discovery through `_NET_CLIENT_LIST` (with a query-tree
//!   fallback) and window-content capture;
//! - exact per-monitor pixel rectangles through RandR `GetMonitors`, which is
//!   what makes mixed-DPI crops pixel-exact;
//! - pointer compositing through XFixes when requested.
//!
//! XShm is not used yet: `GetImage` keeps the crate free of `unsafe` and the
//! transfer cost is small compared with PNG/JPEG encoding at 4K. The capture
//! path is structured so a shared-memory transfer can replace it later without
//! changing callers.

use snipunix_core::{ImageRect, ImageSize};
use thiserror::Error;
use tracing::{debug, warn};
use x11rb::connection::Connection;
use x11rb::image::{Image, PixelLayout};
use x11rb::protocol::randr::ConnectionExt as RandrConnectionExt;
use x11rb::protocol::xfixes::ConnectionExt as XfixesConnectionExt;
use x11rb::protocol::xproto::{Atom, AtomEnum, ConnectionExt as XprotoConnectionExt, Window};
use x11rb::rust_connection::RustConnection;

use crate::image::CapturedImage;
use crate::{CaptureBackend, CaptureError, CaptureFrame, CaptureFuture, CaptureOptions};

/// Pixel rectangle occupied by one monitor in an X11 capture.
#[derive(Debug, Clone, PartialEq)]
pub struct PixelMonitor {
    /// RandR output/monitor name (`eDP-1`, `DP-1`, …), or `None` when the
    /// server only exposed one anonymous screen.
    pub name: Option<String>,
    /// Rectangle in captured-image pixels.
    pub rect: ImageRect,
}

impl PixelMonitor {
    /// Converts to the toolkit-independent monitor-to-image mapping type.
    #[must_use]
    pub fn image_rect(&self) -> snipunix_core::MonitorImage {
        snipunix_core::MonitorImage {
            // X11 does not expose a stable index; callers match by name and
            // fall back to list order.
            id: 0,
            logical: snipunix_core::LogicalRect::new(
                f64::from(self.rect.x),
                f64::from(self.rect.y),
                f64::from(self.rect.width),
                f64::from(self.rect.height),
            ),
            image: self.rect,
        }
    }
}

/// One capturable top-level window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct X11Window {
    /// X window ID.
    pub id: u32,
    /// `_NET_WM_NAME` (UTF-8) or `WM_NAME`.
    pub title: String,
    /// `WM_CLASS` class name.
    pub class: String,
    /// Window geometry in root-window pixels.
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl X11Window {
    /// Visible part of the window, clipped to the screen. `None` when the
    /// window is entirely off-screen.
    #[must_use]
    pub fn visible_rect(&self, screen: ImageSize) -> Option<ImageRect> {
        let left = self.x.max(0) as u32;
        let top = self.y.max(0) as u32;
        let right = (self.x + self.width as i32).clamp(0, screen.width as i32) as u32;
        let bottom = (self.y + self.height as i32).clamp(0, screen.height as i32) as u32;
        let width = right.saturating_sub(left);
        let height = bottom.saturating_sub(top);
        (width > 0 && height > 0).then_some(ImageRect::new(left, top, width, height))
    }
}

/// X11 capture backend.
#[derive(Debug, Default, Clone, Copy)]
pub struct X11Capture;

impl X11Capture {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Captures the whole root window synchronously.
    pub fn capture_root(&self, options: &CaptureOptions) -> Result<CaptureFrame, CaptureError> {
        let connection = X11Connection::connect()?;
        connection.capture_root(options)
    }

    /// Lists capturable top-level windows.
    pub fn list_windows(&self) -> Result<Vec<X11Window>, CaptureError> {
        let connection = X11Connection::connect()?;
        connection.list_windows()
    }

    /// Telemetry-free X11 diagnostics.
    pub fn diagnostics(&self) -> Result<String, CaptureError> {
        X11Connection::connect()?.diagnostics()
    }

    /// Captures one window synchronously.
    pub fn capture_window(
        &self,
        window: u32,
        options: &CaptureOptions,
    ) -> Result<CaptureFrame, CaptureError> {
        let connection = X11Connection::connect()?;
        connection.capture_window(window, options)
    }
}

impl CaptureBackend for X11Capture {
    fn capture(&self, options: CaptureOptions) -> CaptureFuture<'_> {
        Box::pin(async move {
            match options.window {
                Some(window) => self.capture_window(window, &options),
                None => self.capture_root(&options),
            }
        })
    }
}

/// One live X11 connection bound to the default screen.
pub struct X11Connection {
    connection: RustConnection,
    screen: usize,
}

impl X11Connection {
    /// Connects to `$DISPLAY`.
    pub fn connect() -> Result<Self, CaptureError> {
        if std::env::var_os("DISPLAY").is_none() {
            return Err(CaptureError::X11Unavailable(
                "DISPLAY is not set; no X server to capture".to_string(),
            ));
        }
        let (connection, screen) = RustConnection::connect(None)
            .map_err(|error| CaptureError::X11Unavailable(error.to_string()))?;
        debug!(screen, "connected to the X11 server");
        Ok(Self { connection, screen })
    }

    fn root(&self) -> Result<(Window, u16, u16), CaptureError> {
        let screen = &self.connection.setup().roots[self.screen];
        Ok((screen.root, screen.width_in_pixels, screen.height_in_pixels))
    }

    /// Captures the root window.
    pub fn capture_root(&self, options: &CaptureOptions) -> Result<CaptureFrame, CaptureError> {
        let (root, width, height) = self.root()?;
        let width = clamp_dimension(width)?;
        let height = clamp_dimension(height)?;
        let mut image = self.get_image(root, 0, 0, width, height)?;
        if options.include_cursor {
            self.composite_cursor(
                &mut image,
                (0, 0),
                ImageSize::new(u32::from(width), u32::from(height)),
            );
        }
        let monitors = self.monitor_rects(width, height);
        let size = ImageSize::new(u32::from(width), u32::from(height));
        let decoded = CapturedImage::new(
            size.width,
            size.height,
            image,
            Some(format!("x11:root:{width}x{height}")),
        )
        .map_err(CaptureError::Image)?;
        Ok(CaptureFrame {
            image: decoded,
            monitors,
        })
    }

    /// Captures one top-level window.
    pub fn capture_window(
        &self,
        window: u32,
        options: &CaptureOptions,
    ) -> Result<CaptureFrame, CaptureError> {
        let attributes = self
            .connection
            .get_window_attributes(window)
            .map_err(|error| CaptureError::X11(error.to_string()))?
            .reply()
            .map_err(|error| CaptureError::X11(error.to_string()))?;
        if attributes.map_state != x11rb::protocol::xproto::MapState::VIEWABLE {
            return Err(CaptureError::X11(format!(
                "window {window:#x} is not viewable"
            )));
        }
        let geometry = self
            .connection
            .get_geometry(window)
            .map_err(|error| CaptureError::X11(error.to_string()))?
            .reply()
            .map_err(|error| CaptureError::X11(error.to_string()))?;
        if geometry.width == 0 || geometry.height == 0 {
            return Err(CaptureError::X11(format!(
                "window {window:#x} has no drawable area"
            )));
        }

        let width = clamp_dimension(geometry.width)?;
        let height = clamp_dimension(geometry.height)?;
        let (root, screen_width, screen_height) = self.root()?;

        let translated = self
            .translate(window, root)
            .map_err(|error| CaptureError::X11(error.to_string()))?;
        let origin_x = i32::from(translated.0);
        let origin_y = i32::from(translated.1);
        let screen = ImageSize::new(u32::from(screen_width), u32::from(screen_height));

        // Capture the window drawable itself. For a partially off-screen
        // window the server returns what it has. If the drawable cannot be
        // read, compose the visible root area into a full-size window buffer so
        // the result keeps the window geometry and leaves off-screen pixels
        // black.
        let mut rgba = match self.get_image(window, 0, 0, width, height) {
            Ok(pixels) => pixels,
            Err(error) => {
                warn!(%error, window = format!("{window:#x}"), "window drawable capture failed; using the visible root area");
                let visible = X11Window {
                    id: window,
                    title: String::new(),
                    class: String::new(),
                    x: origin_x,
                    y: origin_y,
                    width: u32::from(width),
                    height: u32::from(height),
                }
                .visible_rect(screen);
                let Some(visible) = visible else {
                    return Err(CaptureError::X11(format!(
                        "window {window:#x} is entirely off-screen"
                    )));
                };
                let source = self.get_image(
                    root,
                    visible.x as i16,
                    visible.y as i16,
                    visible.width as u16,
                    visible.height as u16,
                )?;
                let mut buffer = vec![0u8; usize::from(width) * usize::from(height) * 4];
                blit(
                    &mut buffer,
                    u32::from(width),
                    &source,
                    visible.width,
                    visible.height,
                    (visible.x as i32 - origin_x) as u32,
                    (visible.y as i32 - origin_y) as u32,
                );
                buffer
            }
        };
        if options.include_cursor {
            self.composite_cursor(&mut rgba, (origin_x, origin_y), screen);
        }
        let decoded = CapturedImage::new(
            u32::from(width),
            u32::from(height),
            rgba,
            Some(format!("x11:window:{window:#x}")),
        )
        .map_err(CaptureError::Image)?;
        Ok(CaptureFrame {
            image: decoded,
            monitors: Vec::new(),
        })
    }

    /// Telemetry-free summary of the X11 screens, monitors, and extensions.
    pub fn diagnostics(&self) -> Result<String, CaptureError> {
        let (root, width, height) = self.root()?;
        let mut out = String::new();
        out.push_str(&format!("X11 root: {width}x{height} (window {root:#x})\n"));
        let monitors = self.monitor_rects(width, height);
        out.push_str(&format!("X11 monitors: {}\n", monitors.len()));
        for monitor in monitors {
            out.push_str(&format!(
                "  - {}: {}x{}+{}+{}\n",
                monitor.name.as_deref().unwrap_or("anonymous"),
                monitor.rect.width,
                monitor.rect.height,
                monitor.rect.x,
                monitor.rect.y,
            ));
        }
        let xfixes = if self
            .connection
            .xfixes_get_cursor_image()
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some()
        {
            "available"
        } else {
            "unavailable"
        };
        out.push_str(&format!("XFixes cursor: {xfixes}\n"));
        Ok(out)
    }

    /// Lists capturable top-level windows.
    pub fn list_windows(&self) -> Result<Vec<X11Window>, CaptureError> {
        let (root, screen_width, screen_height) = self.root()?;
        let client_list = self.atom(b"_NET_CLIENT_LIST")?;
        let mut ids = Vec::new();

        let client_list_reply = self
            .connection
            .get_property(false, root, client_list, AtomEnum::WINDOW, 0, 4096)
            .ok()
            .and_then(|cookie| cookie.reply().ok());
        if let Some(reply) = client_list_reply {
            if reply.format == 32 {
                for chunk in reply.value32().into_iter().flatten() {
                    ids.push(chunk);
                }
            }
        }
        if ids.is_empty() {
            // Fallback without a window manager: walk the root's children.
            let tree = self
                .connection
                .query_tree(root)
                .map_err(|error| CaptureError::X11(error.to_string()))?
                .reply()
                .map_err(|error| CaptureError::X11(error.to_string()))?;
            ids.extend(tree.children);
        }

        let screen = ImageSize::new(u32::from(screen_width), u32::from(screen_height));
        let mut windows = Vec::new();
        for id in ids {
            let Some(attributes) = self
                .connection
                .get_window_attributes(id)
                .ok()
                .and_then(|cookie| cookie.reply().ok())
            else {
                continue;
            };
            if attributes.override_redirect
                || attributes.map_state != x11rb::protocol::xproto::MapState::VIEWABLE
            {
                continue;
            }
            let Some(geometry) = self
                .connection
                .get_geometry(id)
                .ok()
                .and_then(|cookie| cookie.reply().ok())
            else {
                continue;
            };
            if geometry.width < 16 || geometry.height < 16 {
                continue;
            }
            let Ok((x, y)) = self.translate(id, root) else {
                continue;
            };
            let window = X11Window {
                id,
                title: self
                    .window_title(id)
                    .unwrap_or_else(|| "(untitled)".to_string()),
                class: self.window_class(id).unwrap_or_default(),
                x: i32::from(x),
                y: i32::from(y),
                width: u32::from(geometry.width),
                height: u32::from(geometry.height),
            };
            if window.visible_rect(screen).is_some() {
                windows.push(window);
            }
        }
        Ok(windows)
    }

    fn translate(&self, source: Window, destination: Window) -> Result<(i16, i16), X11Error> {
        let reply = self
            .connection
            .translate_coordinates(source, destination, 0, 0)
            .map_err(|error| X11Error::Connection(error.to_string()))?
            .reply()
            .map_err(|error| X11Error::Connection(error.to_string()))?;
        Ok((reply.dst_x, reply.dst_y))
    }

    fn atom(&self, name: &[u8]) -> Result<Atom, X11Error> {
        self.connection
            .intern_atom(false, name)
            .map_err(|error| X11Error::Connection(error.to_string()))?
            .reply()
            .map(|reply| reply.atom)
            .map_err(|error| X11Error::Connection(error.to_string()))
    }

    fn window_title(&self, window: Window) -> Option<String> {
        let net_wm_name = self.atom(b"_NET_WM_NAME").ok()?;
        let utf8 = self.atom(b"UTF8_STRING").ok()?;
        if let Some(text) = self.read_text_property(window, net_wm_name, utf8) {
            return Some(text);
        }
        let wm_name = AtomEnum::WM_NAME.into();
        self.read_text_property(window, wm_name, AtomEnum::ANY.into())
            .or_else(|| self.read_text_property(window, wm_name, AtomEnum::STRING.into()))
    }

    fn read_text_property(&self, window: Window, property: Atom, type_: Atom) -> Option<String> {
        let reply = self
            .connection
            .get_property(false, window, property, type_, 0, 1024)
            .ok()?
            .reply()
            .ok()?;
        if reply.format != 8 || reply.value.is_empty() {
            return None;
        }
        String::from_utf8(reply.value)
            .ok()
            .map(|text| text.trim_matches('\u{0}').to_string())
            .filter(|text| !text.is_empty())
    }

    fn window_class(&self, window: Window) -> Option<String> {
        let wm_class = x11rb::properties::WmClass::get(&self.connection, window)
            .ok()?
            .reply()
            .ok()??;
        Some(String::from_utf8_lossy(wm_class.class()).to_string())
            .filter(|class| !class.is_empty())
    }

    fn get_image(
        &self,
        drawable: Window,
        x: i16,
        y: i16,
        width: u16,
        height: u16,
    ) -> Result<Vec<u8>, CaptureError> {
        let (image, visual) = Image::get(&self.connection, drawable, x, y, width, height)
            .map_err(|error| CaptureError::X11(error.to_string()))?;
        let layout = self.pixel_layout(visual)?;
        let mut rgba = Vec::with_capacity(usize::from(width) * usize::from(height) * 4);
        for row in 0..height {
            for column in 0..width {
                let (red, green, blue) = layout.decode(image.get_pixel(column, row));
                rgba.extend_from_slice(&[
                    (red >> 8) as u8,
                    (green >> 8) as u8,
                    (blue >> 8) as u8,
                    255,
                ]);
            }
        }
        Ok(rgba)
    }

    fn pixel_layout(&self, visual_id: u32) -> Result<PixelLayout, CaptureError> {
        let screen = &self.connection.setup().roots[self.screen];
        for depth in &screen.allowed_depths {
            for visual in &depth.visuals {
                if visual.visual_id == visual_id {
                    return PixelLayout::from_visual_type(*visual)
                        .map_err(|error| CaptureError::X11(error.to_string()));
                }
            }
        }
        Err(CaptureError::X11(format!(
            "unknown X11 visual {visual_id:#x}"
        )))
    }

    fn composite_cursor(&self, rgba: &mut [u8], origin: (i32, i32), image_size: ImageSize) {
        match self.cursor() {
            Ok(Some(cursor)) => cursor.composite(rgba, origin, image_size),
            Ok(None) => {}
            Err(error) => debug!(%error, "XFixes cursor is unavailable; capturing without it"),
        }
    }

    fn cursor(&self) -> Result<Option<CursorImage>, X11Error> {
        let reply = match self.connection.xfixes_get_cursor_image() {
            Ok(cookie) => cookie
                .reply()
                .map_err(|error| X11Error::Connection(error.to_string()))?,
            Err(error) => return Err(X11Error::Unavailable(error.to_string())),
        };
        let width = usize::from(reply.width);
        let height = usize::from(reply.height);
        if width == 0 || height == 0 || reply.cursor_image.len() < width * height {
            return Ok(None);
        }
        Ok(Some(CursorImage {
            x: i32::from(reply.x) - i32::from(reply.xhot),
            y: i32::from(reply.y) - i32::from(reply.yhot),
            width,
            height,
            pixels: reply.cursor_image,
        }))
    }

    fn monitor_rects(&self, width: u16, height: u16) -> Vec<PixelMonitor> {
        let fallback = || {
            vec![PixelMonitor {
                name: None,
                rect: ImageRect::new(0, 0, u32::from(width), u32::from(height)),
            }]
        };
        let (root, _, _) = match self.root() {
            Ok(root) => root,
            Err(_) => return fallback(),
        };
        let reply = match self.connection.randr_get_monitors(root, true) {
            Ok(cookie) => cookie.reply(),
            Err(error) => {
                debug!(%error, "RandR GetMonitors is unavailable");
                return fallback();
            }
        };
        let Ok(reply) = reply else {
            return fallback();
        };
        let mut monitors = Vec::with_capacity(reply.monitors.len());
        for monitor in reply.monitors {
            let Some(rect) = rect_from_i16(monitor.x, monitor.y, monitor.width, monitor.height)
            else {
                continue;
            };
            if rect.width == 0 || rect.height == 0 {
                continue;
            }
            let name = self
                .connection
                .get_atom_name(monitor.name)
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .map(|reply| String::from_utf8_lossy(&reply.name).to_string())
                .filter(|name| !name.is_empty());
            monitors.push(PixelMonitor { name, rect });
        }
        if monitors.is_empty() {
            fallback()
        } else {
            monitors
        }
    }
}

#[derive(Debug, Error)]
enum X11Error {
    #[error("{0}")]
    Connection(String),
    #[error("{0}")]
    Unavailable(String),
}

impl From<X11Error> for CaptureError {
    fn from(error: X11Error) -> Self {
        match error {
            X11Error::Connection(message) => CaptureError::X11(message),
            X11Error::Unavailable(message) => CaptureError::X11Unavailable(message),
        }
    }
}

struct CursorImage {
    x: i32,
    y: i32,
    width: usize,
    height: usize,
    pixels: Vec<u32>,
}

impl CursorImage {
    fn composite(&self, rgba: &mut [u8], origin: (i32, i32), screen: ImageSize) {
        for row in 0..self.height {
            let destination_y = self.y - origin.1 + row as i32;
            if destination_y < 0 || destination_y >= screen.height as i32 {
                continue;
            }
            for column in 0..self.width {
                let destination_x = self.x - origin.0 + column as i32;
                if destination_x < 0 || destination_x >= screen.width as i32 {
                    continue;
                }
                let source = self.pixels[row * self.width + column];
                let alpha = u32::from((source >> 24) as u8);
                if alpha == 0 {
                    continue;
                }
                let index =
                    ((destination_y as u32 * screen.width + destination_x as u32) * 4) as usize;
                let Some(pixel) = rgba.get_mut(index..index + 4) else {
                    continue;
                };
                let source_red = u32::from((source >> 16) as u8);
                let source_green = u32::from((source >> 8) as u8);
                let source_blue = u32::from(source as u8);
                for (channel, value) in [source_red, source_green, source_blue]
                    .into_iter()
                    .enumerate()
                {
                    let blended = (value * alpha + u32::from(pixel[channel]) * (255 - alpha)) / 255;
                    pixel[channel] = blended as u8;
                }
            }
        }
    }
}

fn clamp_dimension(value: u16) -> Result<u16, CaptureError> {
    if value == 0 {
        return Err(CaptureError::X11(
            "the X server reported a zero-sized drawable".to_string(),
        ));
    }
    Ok(value)
}

fn rect_from_i16(x: i16, y: i16, width: u16, height: u16) -> Option<ImageRect> {
    let x = u32::try_from(x).ok()?;
    let y = u32::try_from(y).ok()?;
    Some(ImageRect::new(x, y, u32::from(width), u32::from(height)))
}

/// Copies a tightly packed RGBA source into a larger RGBA destination.
fn blit(
    destination: &mut [u8],
    destination_width: u32,
    source: &[u8],
    source_width: u32,
    source_height: u32,
    x: u32,
    y: u32,
) {
    let source_row = usize::try_from(source_width * 4).unwrap_or(0);
    let destination_row = usize::try_from(destination_width * 4).unwrap_or(0);
    for row in 0..usize::try_from(source_height).unwrap_or(0) {
        let source_start = row * source_row;
        let destination_start = (usize::try_from(y).unwrap_or(0) + row) * destination_row
            + usize::try_from(x * 4).unwrap_or(0);
        let Some(destination_slice) =
            destination.get_mut(destination_start..destination_start + source_row)
        else {
            continue;
        };
        let Some(source_slice) = source.get(source_start..source_start + source_row) else {
            continue;
        };
        destination_slice.copy_from_slice(source_slice);
    }
}

/// Reads the X11 `_NET_CLIENT_LIST`-style windows without capturing.
pub fn discover_windows() -> Result<Vec<X11Window>, CaptureError> {
    X11Capture::new().list_windows()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_rect_clips_partially_offscreen_windows() {
        let window = X11Window {
            id: 1,
            title: "test".into(),
            class: "Test".into(),
            x: -50,
            y: -20,
            width: 200,
            height: 100,
        };
        assert_eq!(
            window.visible_rect(ImageSize::new(1280, 800)),
            Some(ImageRect::new(0, 0, 150, 80))
        );
    }

    #[test]
    fn visible_rect_rejects_offscreen_windows() {
        let window = X11Window {
            id: 1,
            title: String::new(),
            class: String::new(),
            x: 2000,
            y: 0,
            width: 100,
            height: 100,
        };
        assert_eq!(window.visible_rect(ImageSize::new(1280, 800)), None);
    }

    #[test]
    fn blit_places_source_at_the_requested_offset() {
        let mut destination = vec![0u8; 4 * 4 * 4];
        let source = vec![7u8; 2 * 2 * 4];
        blit(&mut destination, 4, &source, 2, 2, 1, 1);
        assert_eq!(&destination[(1 * 4 + 1) * 4..(1 * 4 + 1) * 4 + 4], &[7; 4]);
        assert_eq!(&destination[0..4], &[0; 4]);
    }
}
