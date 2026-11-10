//! Screenshots (`ScreenSource`) through ScreenCaptureKit.
//!
//! Each call runs on a blocking-pool thread: ScreenCaptureKit answers on its own queues, and the
//! completion handlers hand their result back over a channel, so no Objective-C object is held
//! across an await. Laqta's own application is left out of display captures with the content
//! filter, so its overlays and toasts never show up in a screenshot.

use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use laqta_domain::geometry::{Monitor, MonitorId};
use laqta_domain::region::HoverTarget;
use laqta_domain::{Bitmap, Frame, PixelFormat};
use laqta_ports::{BoxFuture, CaptureRequest, PortError, ScreenSource};
use objc2::AnyThread;
use objc2::rc::{Retained, autoreleasepool};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo, kCGColorSpaceSRGB,
};
use objc2_core_video::kCVPixelFormatType_32BGRA;
use objc2_foundation::{NSArray, NSError};
use objc2_screen_capture_kit::{
    SCContentFilter, SCRunningApplication, SCScreenshotManager, SCShareableContent, SCStreamConfiguration, SCStreamErrorCode,
    SCStreamErrorDomain,
};

use crate::displays::{self, Display};
use crate::permissions;

/// How long to wait for ScreenCaptureKit to answer, per request.
const WAIT: Duration = Duration::from_secs(5);

#[derive(Debug, Default)]
pub struct MacScreen;

impl ScreenSource for MacScreen {
    fn capture_all(&self) -> BoxFuture<'_, Result<Vec<Frame>, PortError>> {
        self.capture(CaptureRequest::default())
    }

    fn capture(&self, request: CaptureRequest) -> BoxFuture<'_, Result<Vec<Frame>, PortError>> {
        Box::pin(blocking::unblock(move || autoreleasepool(|_| capture_displays(request))))
    }

    fn capture_window(&self, target: &HoverTarget, request: CaptureRequest) -> BoxFuture<'_, Result<Bitmap, PortError>> {
        let handle = target.handle;
        Box::pin(async move {
            if handle == 0 {
                return Err(PortError::Unsupported("window capture without a window id".into()));
            }
            blocking::unblock(move || autoreleasepool(|_| capture_window_bitmap(handle, request))).await
        })
    }
}

fn failed(message: impl Into<String>) -> PortError {
    PortError::Failed(message.into())
}

/// One frame per active display, in `displays::list` order.
fn capture_displays(request: CaptureRequest) -> Result<Vec<Frame>, PortError> {
    permissions::screen_recording()?;
    let displays = displays::list();
    if displays.is_empty() {
        return Err(failed("no active display"));
    }
    let content = shareable_content()?;
    // SAFETY: a plain getter on a live object.
    let sc_displays = unsafe { content.displays() }.to_vec();
    let own_apps = NSArray::from_retained_slice(&own_applications(&content));
    let no_windows = NSArray::from_retained_slice(&[]);

    let mut frames = Vec::with_capacity(displays.len());
    for display in &displays {
        // SAFETY: plain getter on a live object.
        let Some(sc_display) = sc_displays.iter().find(|d| unsafe { d.displayID() } == display.id) else {
            return Err(failed(format!("display {} is not available for capture", display.id)));
        };
        // SAFETY: the display and both arrays are alive for the call; `alloc` yields a fresh object.
        let filter = unsafe {
            SCContentFilter::initWithDisplay_excludingApplications_exceptingWindows(
                SCContentFilter::alloc(),
                sc_display,
                &own_apps,
                &no_windows,
            )
        };
        let rect = display.phys_rect();
        let config = configuration(rect.width as usize, rect.height as usize, request.show_cursor);
        let image = capture_image(&filter, &config, true)?;
        frames.push(Frame { monitor: monitor(display), image });
    }
    Ok(frames)
}

/// A window by itself, with its shadow and rounded corners and a real alpha channel.
fn capture_window_bitmap(handle: u64, request: CaptureRequest) -> Result<Bitmap, PortError> {
    let id = u32::try_from(handle).map_err(|_| failed("not a window id"))?;
    permissions::screen_recording()?;
    let content = shareable_content()?;
    // SAFETY: plain getters on live objects.
    let Some(window) = unsafe { content.windows() }.to_vec().into_iter().find(|w| unsafe { w.windowID() } == id) else {
        return Err(failed("the window is gone"));
    };
    // SAFETY: the window is alive for the call; `alloc` yields a fresh object.
    let filter = unsafe { SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), &window) };

    // The filter knows the full extent in points, shadow included. The frame is the fallback.
    // SAFETY: plain getters on live objects.
    let (extent, point_pixel_scale, frame) = unsafe {
        let info = SCShareableContent::infoForFilter(&filter);
        (info.contentRect(), info.pointPixelScale(), window.frame())
    };
    let (width, height) = if extent.size.width > 0.0 && extent.size.height > 0.0 && point_pixel_scale > 0.0 {
        let scale = f64::from(point_pixel_scale);
        ((extent.size.width * scale).round() as usize, (extent.size.height * scale).round() as usize)
    } else {
        let rect = displays::to_phys(&displays::list(), frame.origin.x, frame.origin.y, frame.size.width, frame.size.height);
        (rect.width as usize, rect.height as usize)
    };
    let config = configuration(width, height, request.show_cursor);
    capture_image(&filter, &config, false)
}

fn configuration(width: usize, height: usize, show_cursor: bool) -> Retained<SCStreamConfiguration> {
    // SAFETY: a fresh configuration object that only this function uses; the setters take plain values.
    unsafe {
        let config = SCStreamConfiguration::new();
        config.setWidth(width);
        config.setHeight(height);
        config.setShowsCursor(show_cursor);
        config.setPixelFormat(kCVPixelFormatType_32BGRA);
        config
    }
}

fn monitor(display: &Display) -> Monitor {
    Monitor {
        id: MonitorId(display.id.to_string()),
        name: display.name.clone(),
        rect: display.phys_rect(),
        scale: display.scale as f32,
        primary: display.primary,
    }
}

/// ScreenCaptureKit's record of this process, to leave it out of a display capture. Empty when
/// the process has no windows yet.
fn own_applications(content: &SCShareableContent) -> Vec<Retained<SCRunningApplication>> {
    let pid = std::process::id() as i32;
    // SAFETY: plain getters on live objects.
    unsafe { content.applications() }.to_vec().into_iter().filter(|app| unsafe { app.processID() } == pid).collect()
}

fn shareable_content() -> Result<Retained<SCShareableContent>, PortError> {
    let (tx, rx) = mpsc::channel();
    let handler = RcBlock::new(move |content: *mut SCShareableContent, error: *mut NSError| {
        // SAFETY: both pointers are null or valid for the duration of the call; retaining keeps the content past it.
        let result = unsafe {
            match Retained::retain(content) {
                Some(content) => Ok(content),
                None => Err(sck_error(error.as_ref())),
            }
        };
        let _ = tx.send(result);
    });
    // SAFETY: the block copies itself; ScreenCaptureKit calls it once, on any thread, and the
    // content it hands over is only used again on this thread.
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&handler) };
    rx.recv_timeout(WAIT).map_err(|_| failed("ScreenCaptureKit did not answer in time"))?
}

/// Takes the screenshot and converts it to a `Bitmap`. `opaque` forces alpha to 255 (displays);
/// otherwise the image keeps its alpha (windows).
fn capture_image(filter: &SCContentFilter, config: &SCStreamConfiguration, opaque: bool) -> Result<Bitmap, PortError> {
    let (tx, rx) = mpsc::channel();
    let handler = RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
        // SAFETY: both pointers are null or valid for the duration of the call, which is all the conversion needs.
        let result = unsafe {
            match image.as_ref() {
                Some(image) => bitmap_from_image(image, opaque),
                None => Err(sck_error(error.as_ref())),
            }
        };
        let _ = tx.send(result);
    });
    // SAFETY: the filter and configuration are alive for the call, and the block copies itself.
    unsafe { SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(filter, config, Some(&handler)) };
    rx.recv_timeout(WAIT).map_err(|_| failed("ScreenCaptureKit did not answer in time"))?
}

/// Draws the image into a tightly packed sRGB BGRA buffer. Drawing (rather than reading the image's
/// own bytes) removes any row padding and converts the display's colour space to sRGB.
fn bitmap_from_image(image: &CGImage, opaque: bool) -> Result<Bitmap, PortError> {
    let (width, height) = (CGImage::width(Some(image)), CGImage::height(Some(image)));
    let (Ok(w), Ok(h)) = (u32::try_from(width), u32::try_from(height)) else {
        return Err(failed("the screenshot is too large"));
    };
    if width == 0 || height == 0 {
        return Err(failed("the screenshot is empty"));
    }
    let row = width.checked_mul(4).ok_or_else(|| failed("the screenshot is too large"))?;
    let mut pixels = vec![0u8; row.checked_mul(height).ok_or_else(|| failed("the screenshot is too large"))?];

    // SAFETY: reading an immutable constant.
    let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })).ok_or_else(|| failed("no sRGB colour space"))?;
    // Premultiplied alpha with little-endian 32-bit words is BGRA in memory.
    let info = CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0;
    // SAFETY: `pixels` holds `height` rows of `row` bytes and is neither moved nor touched until
    // the context is dropped below.
    let context = unsafe { CGBitmapContextCreate(pixels.as_mut_ptr().cast(), width, height, 8, row, Some(&space), info) }
        .ok_or_else(|| failed("could not create a bitmap context"))?;
    let rect = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(width as f64, height as f64));
    CGContext::draw_image(Some(&context), rect, Some(image));
    drop(context);

    if opaque {
        displays::make_opaque_bgra(&mut pixels);
    } else {
        displays::unpremultiply_bgra(&mut pixels);
    }
    Bitmap::new(w, h, PixelFormat::Bgra8, pixels.into()).map_err(|e| failed(e.to_string()))
}

/// Maps a ScreenCaptureKit error. "User declined" means Screen Recording is not granted.
fn sck_error(error: Option<&NSError>) -> PortError {
    let Some(error) = error else {
        return failed("ScreenCaptureKit returned no image");
    };
    // SAFETY: reading an immutable constant.
    let domain = unsafe { SCStreamErrorDomain };
    if *error.domain() == *domain && error.code() == SCStreamErrorCode::UserDeclined.0 {
        return PortError::PermissionDenied;
    }
    failed(format!("ScreenCaptureKit: {}", error.localizedDescription()))
}
