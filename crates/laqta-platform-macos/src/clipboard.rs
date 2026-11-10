//! The general pasteboard (`Clipboard`).
//!
//! Images are written by hand as PNG and TIFF: arboard's `set_image` puts an `NSImage` on the
//! pasteboard, which carries TIFF only, and some apps read only PNG. Text, files and reads go through
//! arboard, which uses the same `NSPasteboard`. All pasteboard calls run on the main thread.

use std::ptr;

use laqta_domain::{Bitmap, PixelFormat};
use laqta_ports::{Clipboard, ClipboardContent, PortError};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AllocAnyThread, ClassType, msg_send};
use objc2_app_kit::{
    NSBitmapFormat, NSBitmapImageFileType, NSBitmapImageRep, NSDeviceRGBColorSpace, NSPasteboard, NSPasteboardTypePNG, NSPasteboardTypeTIFF,
};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSInteger, NSString};

use crate::main_thread::on_main;

#[derive(Debug, Default)]
pub struct MacClipboard;

impl Clipboard for MacClipboard {
    fn set_image(&self, image: &Bitmap) -> Result<(), PortError> {
        // Encoding a large screenshot takes a while, so it stays off the main thread.
        let (png, tiff) = encode(image)?;
        on_main(move |_| write_image(&png, &tiff))
    }

    fn set_text(&self, text: &str) -> Result<(), PortError> {
        on_main(|_| arboard::Clipboard::new().and_then(|mut cb| cb.set_text(text)).map_err(arboard_error))
    }

    fn set_files(&self, paths: &[std::path::PathBuf]) -> Result<(), PortError> {
        on_main(|_| arboard::Clipboard::new().and_then(|mut cb| cb.set().file_list(paths)).map_err(arboard_error))
    }

    fn get(&self) -> Result<Option<ClipboardContent>, PortError> {
        on_main(|_| {
            let mut cb = arboard::Clipboard::new().map_err(arboard_error)?;
            read_clipboard(&mut cb)
        })
    }
}

fn arboard_error(e: arboard::Error) -> PortError {
    PortError::Failed(format!("clipboard: {e}"))
}

/// Same order as the other platforms: image, then files, then text.
fn read_clipboard(cb: &mut arboard::Clipboard) -> Result<Option<ClipboardContent>, PortError> {
    if let Ok(img) = cb.get_image() {
        let bmp = Bitmap::new(img.width as u32, img.height as u32, PixelFormat::Rgba8, std::sync::Arc::from(img.bytes.into_owned()))
            .map_err(|e| PortError::Failed(format!("clipboard image: {e}")))?;
        return Ok(Some(ClipboardContent::Image(bmp)));
    }
    if let Ok(files) = cb.get().file_list()
        && !files.is_empty()
    {
        return Ok(Some(ClipboardContent::Files(files)));
    }
    match cb.get_text() {
        Ok(t) if !t.is_empty() => Ok(Some(ClipboardContent::Text(t))),
        _ => Ok(None),
    }
}

/// The PNG and TIFF encodings of `image`, made from one non-premultiplied RGBA bitmap rep.
fn encode(image: &Bitmap) -> Result<(Vec<u8>, Vec<u8>), PortError> {
    let (width, height) = (image.width() as usize, image.height() as usize);
    if width == 0 || height == 0 {
        return Err(PortError::Failed("clipboard: the image is empty".into()));
    }
    let row = width * 4;
    // SAFETY: a constant of the AppKit framework.
    let color_space = unsafe { NSDeviceRGBColorSpace };
    // SAFETY: null planes make the rep allocate its own buffer. The other arguments describe a packed,
    // non-planar bitmap of 8-bit RGB samples plus straight alpha: 4 samples and 32 bits per pixel.
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            ptr::null_mut(),
            width as NSInteger,
            height as NSInteger,
            8,
            4,
            true,
            false,
            color_space,
            NSBitmapFormat::AlphaNonpremultiplied,
            row as NSInteger,
            32,
        )
    }
    .ok_or_else(|| PortError::Failed("clipboard: could not create a bitmap".into()))?;

    let stride = usize::try_from(rep.bytesPerRow()).unwrap_or(0);
    let plane = usize::try_from(rep.bytesPerPlane()).unwrap_or(0);
    let dst = rep.bitmapData();
    if dst.is_null() || stride < row || plane < stride * height {
        return Err(PortError::Failed("clipboard: unexpected bitmap layout".into()));
    }
    for (y, src_row) in image.pixels().chunks_exact(row).enumerate() {
        // SAFETY: `dst` is the rep's own buffer of at least `stride * height` bytes (checked above), `y < height`,
        // and nothing else refers to it while this row slice lives.
        let dst_row = unsafe { std::slice::from_raw_parts_mut(dst.add(y * stride), row) };
        match image.format() {
            PixelFormat::Rgba8 => dst_row.copy_from_slice(src_row),
            PixelFormat::Bgra8 => {
                for (d, s) in dst_row.as_chunks_mut::<4>().0.iter_mut().zip(src_row.as_chunks::<4>().0) {
                    *d = [s[2], s[1], s[0], s[3]];
                }
            }
        }
    }

    let properties = NSDictionary::<NSString, AnyObject>::new();
    // SAFETY: an empty properties dictionary is valid for PNG.
    let png = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &properties) }
        .ok_or_else(|| PortError::Failed("clipboard: could not encode PNG".into()))?;
    let tiff = rep.TIFFRepresentation().ok_or_else(|| PortError::Failed("clipboard: could not encode TIFF".into()))?;
    Ok((png.to_vec(), tiff.to_vec()))
}

/// Replaces the pasteboard contents with one item that has both encodings.
fn write_image(png: &[u8], tiff: &[u8]) -> Result<(), PortError> {
    let pasteboard = general_pasteboard()?;
    // SAFETY: constants of the AppKit framework.
    let (png_type, tiff_type) = unsafe { (NSPasteboardTypePNG, NSPasteboardTypeTIFF) };
    // Declaring the types clears the old contents. The data follows at once, so no owner is needed.
    // SAFETY: `None` is a valid owner.
    unsafe { pasteboard.declareTypes_owner(&NSArray::from_slice(&[png_type, tiff_type]), None) };
    let png_set = pasteboard.setData_forType(Some(&NSData::with_bytes(png)), png_type);
    let tiff_set = pasteboard.setData_forType(Some(&NSData::with_bytes(tiff)), tiff_type);
    if png_set && tiff_set { Ok(()) } else { Err(PortError::Failed("clipboard: the pasteboard refused the image".into())) }
}

/// `generalPasteboard` can return nil in a process with no window-server session, which the typed
/// binding does not allow for.
fn general_pasteboard() -> Result<Retained<NSPasteboard>, PortError> {
    // SAFETY: `generalPasteboard` takes no arguments and returns an `NSPasteboard` or nil.
    let pasteboard: Option<Retained<NSPasteboard>> = unsafe { msg_send![NSPasteboard::class(), generalPasteboard] };
    pasteboard.ok_or_else(|| PortError::Failed("clipboard: no general pasteboard".into()))
}
