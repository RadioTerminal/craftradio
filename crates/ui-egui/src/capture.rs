//! Screenshots of the main window on request, for the control channel's `ui.screenshot` (an
//! AI client looking at the app through MCP). craft-ui's `CaptureSlot` pairs one capture with one
//! request; the image comes back as PNG.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};

use craft_ui::capture::CaptureSlot;
use egui::{ColorImage, ViewportCommand, ViewportId};

/// A captured window.
#[derive(Clone, Debug)]
pub struct Screenshot {
    /// The window as a PNG file.
    pub png: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

/// A request for a screenshot of the main window, answered on `reply`.
pub struct ScreenshotRequest {
    pub reply: Sender<Result<Screenshot, String>>,
}

/// The largest capture accepted: a 4K window at four bytes a pixel, twice over.
const MAX_BYTES: usize = 64 << 20;

/// The requests of a window, and the one being captured.
pub(crate) struct Captures {
    requests: Receiver<ScreenshotRequest>,
    pending: Option<(CaptureSlot, Sender<Result<Screenshot, String>>)>,
}

impl Captures {
    pub(crate) fn new(requests: Receiver<ScreenshotRequest>) -> Self {
        Captures {
            requests,
            pending: None,
        }
    }

    /// Once a frame: answers a capture that finished, and starts the next request.
    pub(crate) fn tick(&mut self, ctx: &egui::Context) {
        if let Some((slot, _)) = &self.pending {
            match slot.take() {
                Some(result) => {
                    if let Some((_, reply)) = self.pending.take() {
                        let _ = reply.send(result.map_err(|e| e.to_string()).and_then(png));
                    }
                }
                // The capture arrives with a later frame.
                None => ctx.request_repaint(),
            }
        }
        if self.pending.is_none()
            && let Ok(request) = self.requests.try_recv()
        {
            let (slot, callback) = CaptureSlot::new(ctx, ViewportId::ROOT, MAX_BYTES);
            ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Screenshot(callback));
            self.pending = Some((slot, request.reply));
            ctx.request_repaint();
        }
    }
}

/// `image` as a PNG file.
fn png(image: Arc<ColorImage>) -> Result<Screenshot, String> {
    let [width, height] = image.size;
    let rgba: Vec<u8> = image
        .pixels
        .iter()
        .flat_map(|c| c.to_srgba_unmultiplied())
        .collect();
    let (w, h) = (
        u32::try_from(width).map_err(|e| e.to_string())?,
        u32::try_from(height).map_err(|e| e.to_string())?,
    );
    let buffer =
        image::RgbaImage::from_raw(w, h, rgba).ok_or("the screenshot does not match its size")?;
    let mut out = std::io::Cursor::new(Vec::new());
    buffer
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| format!("cannot write the PNG: {e}"))?;
    Ok(Screenshot {
        png: out.into_inner(),
        width,
        height,
    })
}
