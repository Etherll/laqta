//! DXGI Desktop Duplication stills. One D3D11 device per adapter, so on hybrid-GPU laptops each
//! output is duplicated on the adapter that owns it. Frames never include the cursor. SDR outputs
//! are read as 8-bit BGRA. HDR outputs (scRGB) are duplicated as FP16 and tonemapped to sRGB, with
//! the 8-bit duplication as the fallback when the driver refuses FP16.

use std::mem::size_of;
use std::time::{Duration, Instant};

use laqta_domain::{Bitmap, Frame, PixelFormat};
use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_HEADER,
    DISPLAYCONFIG_DEVICE_INFO_TYPE, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SDR_WHITE_LEVEL,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Foundation::{E_FAIL, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, HMODULE, LUID};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_COLOR_SPACE_TYPE, DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_MODE_ROTATION_IDENTITY,
    DXGI_MODE_ROTATION_UNSPECIFIED, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput, IDXGIOutput1,
    IDXGIOutput5, IDXGIOutput6, IDXGIOutputDuplication, IDXGIResource,
};
use windows::core::{Error, Interface, Result};

use crate::capture_gdi::monitor_from_handle;
use crate::hdr::{tonemap_frame, wants_fp16 as is_hdr_colour_space};

/// How long to wait for a frame whose `LastPresentTime` is set.
const FIRST_FRAME_BUDGET: Duration = Duration::from_millis(500);

/// SDR white in scRGB units when Windows reports no SDR white level: 3.0, which is 240 nits.
const FALLBACK_SDR_WHITE: f32 = 3.0;

/// A GPU staging copy of the last presented frame, kept while its size and format stay the same.
struct Staging {
    texture: ID3D11Texture2D,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
}

/// The frames of one duplication pass, and what Windows said about the pointer while taking them.
pub(crate) struct Captured {
    pub(crate) frames: Vec<Frame>,
    /// Whether the presented frame of any output reported the pointer as visible. Duplication frames
    /// never contain the pointer; this only tells the debug log what Windows had on screen.
    pub(crate) pointer_visible: bool,
    /// For each frame, the SDR white scale of its output when that output shows an HDR desktop.
    /// Windows often draws the pointer into an HDR desktop itself, so duplication can hold it.
    pub(crate) hdr_white: Vec<Option<f32>>,
}

/// With `hdr_tonemap` false, an HDR output is duplicated in 8 bits, so Windows makes the SDR image
/// instead of Laqta's tonemap.
pub(crate) fn capture(hdr_tonemap: bool) -> Result<Captured> {
    // SAFETY: COM calls on interfaces owned by this function; DXGI needs no apartment.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut frames = Vec::new();
        let mut pointer_visible = false;
        let mut hdr_whites = Vec::new();
        let mut adapter_index = 0;
        while let Ok(adapter) = factory.EnumAdapters1(adapter_index) {
            adapter_index += 1;
            let mut outputs = Vec::new();
            while let Ok(output) = adapter.EnumOutputs(outputs.len() as u32) {
                outputs.push(output);
            }
            if outputs.is_empty() {
                continue; // e.g. the Basic Render Driver
            }
            let (device, context) = create_device(&adapter)?;
            for output in outputs {
                let desc = output.GetDesc()?;
                if !desc.AttachedToDesktop.as_bool() {
                    continue;
                }
                // Duplication returns rotated outputs unrotated; GDI handles them correctly.
                if desc.Rotation != DXGI_MODE_ROTATION_IDENTITY && desc.Rotation != DXGI_MODE_ROTATION_UNSPECIFIED {
                    return Err(E_FAIL.into());
                }
                let monitor = monitor_from_handle(desc.Monitor).ok_or_else(|| Error::from(E_FAIL))?;
                let white = output_wants_fp16(&output).then(|| sdr_white_scale(&monitor.name));
                let hdr_white = white.filter(|_| hdr_tonemap);
                let (image, pointer) = duplicate(&output.cast::<IDXGIOutput1>()?, &device, &context, hdr_white, &monitor.name)?;
                pointer_visible |= pointer;
                frames.push(Frame { monitor, image });
                hdr_whites.push(white);
            }
        }
        if frames.is_empty() {
            return Err(E_FAIL.into());
        }
        Ok(Captured { frames, pointer_visible, hdr_white: hdr_whites })
    }
}

/// Whether any output on any adapter shows an HDR desktop. For the log line when capture falls back
/// to GDI, which cannot read HDR pixels.
pub(crate) fn any_hdr_output() -> bool {
    // SAFETY: COM calls on interfaces owned by this function; DXGI needs no apartment.
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else {
            return false;
        };
        let mut adapter_index = 0;
        while let Ok(adapter) = factory.EnumAdapters1(adapter_index) {
            adapter_index += 1;
            let mut output_index = 0;
            while let Ok(output) = adapter.EnumOutputs(output_index) {
                output_index += 1;
                if output_wants_fp16(&output) {
                    return true;
                }
            }
        }
        false
    }
}

/// Whether `cs` is an HDR colour space, which is duplicated as FP16 and tonemapped to sRGB. The
/// decision itself is in `hdr::wants_fp16`, on the raw value, so it is tested on every target.
fn wants_fp16(cs: DXGI_COLOR_SPACE_TYPE) -> bool {
    is_hdr_colour_space(cs.0)
}

/// Whether the output shows an HDR desktop (`wants_fp16` of its colour space). Windows older than
/// 10 1703 has no `IDXGIOutput6`, so those outputs count as SDR.
fn output_wants_fp16(output: &IDXGIOutput) -> bool {
    let Ok(output6) = output.cast::<IDXGIOutput6>() else {
        return false;
    };
    // SAFETY: a plain query on a live output.
    unsafe { output6.GetDesc1() }.is_ok_and(|desc| wants_fp16(desc.ColorSpace))
}

/// SDR white in scRGB units for the monitor named `device`: `SDRWhiteLevel / 1000`, since 1000 is
/// 80 nits and scRGB 1.0 is 80 nits. Falls back to 240 nits when Windows reports no level.
pub(crate) fn sdr_white_scale(device: &str) -> f32 {
    sdr_white_level(device).map_or(FALLBACK_SDR_WHITE, |level| level as f32 / 1000.0)
}

/// The `SDRWhiteLevel` of the active display path whose GDI device name is `device`.
/// The active display paths. The buffers are sized with `GetDisplayConfigBufferSizes`, and the query
/// is retried when the topology changes between the two calls.
fn active_paths() -> Option<Vec<DISPLAYCONFIG_PATH_INFO>> {
    for _ in 0..3 {
        let mut paths = 0u32;
        let mut modes = 0u32;
        // SAFETY: both out-pointers are locals.
        let sized = unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &raw mut paths, &raw mut modes) };
        if sized != ERROR_SUCCESS {
            return None;
        }
        let mut path_info = vec![DISPLAYCONFIG_PATH_INFO::default(); paths as usize];
        let mut mode_info = vec![DISPLAYCONFIG_MODE_INFO::default(); modes as usize];
        // SAFETY: each array has the capacity the sizing call reported, and the counts are in-out.
        let got = unsafe {
            QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &raw mut paths, path_info.as_mut_ptr(), &raw mut modes, mode_info.as_mut_ptr(), None)
        };
        if got == ERROR_SUCCESS {
            path_info.truncate(paths as usize);
            return Some(path_info);
        }
        if got != ERROR_INSUFFICIENT_BUFFER {
            return None;
        }
    }
    None
}

fn sdr_white_level(device: &str) -> Option<u32> {
    active_paths()?.iter().find_map(|path| {
        let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
            header: device_info_header(
                DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>(),
                path.sourceInfo.adapterId,
                path.sourceInfo.id,
            ),
            ..Default::default()
        };
        // SAFETY: the packet's header declares its type and size, and the packet outlives the call.
        if unsafe { DisplayConfigGetDeviceInfo(&raw mut source.header) } != 0 {
            return None;
        }
        if wide_to_string(&source.viewGdiDeviceName) != device {
            return None;
        }
        let mut white = DISPLAYCONFIG_SDR_WHITE_LEVEL {
            header: device_info_header(
                DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL,
                size_of::<DISPLAYCONFIG_SDR_WHITE_LEVEL>(),
                path.targetInfo.adapterId,
                path.targetInfo.id,
            ),
            SDRWhiteLevel: 0,
        };
        // SAFETY: as above; the level is queried on the path's target, not its source.
        if unsafe { DisplayConfigGetDeviceInfo(&raw mut white.header) } != 0 {
            return None;
        }
        Some(white.SDRWhiteLevel).filter(|&level| level > 0)
    })
}

fn device_info_header(kind: DISPLAYCONFIG_DEVICE_INFO_TYPE, size: usize, adapter: LUID, id: u32) -> DISPLAYCONFIG_DEVICE_INFO_HEADER {
    DISPLAYCONFIG_DEVICE_INFO_HEADER { r#type: kind, size: size as u32, adapterId: adapter, id }
}

/// A NUL-terminated UTF-16 buffer as a `String`, lossily.
fn wide_to_string(wide: &[u16]) -> String {
    let len = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..len])
}

unsafe fn create_device(adapter: &IDXGIAdapter1) -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device = None;
    let mut context = None;
    // SAFETY: out-pointers to locals; a real adapter requires D3D_DRIVER_TYPE_UNKNOWN.
    unsafe {
        D3D11CreateDevice(
            adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
    }
    match (device, context) {
        (Some(d), Some(c)) => Ok((d, c)),
        _ => Err(E_FAIL.into()),
    }
}

/// Acquires frames until one has `LastPresentTime != 0`, for at most 500 ms, and copies it into a
/// staging texture. Frames with `LastPresentTime == 0` can be blank on some drivers, so a timeout
/// fails and the caller falls back to GDI.
///
/// With `hdr_white` set (an HDR output), FP16 duplication is tried first. If the driver refuses
/// it, the 8-bit duplication is used. The format of the texture that was acquired decides whether
/// the frame is tonemapped. `name` is the monitor's device name, for the log.
///
/// The second value is whether the presented frame reported the pointer as visible.
unsafe fn duplicate(
    output: &IDXGIOutput1,
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    hdr_white: Option<f32>,
    name: &str,
) -> Result<(Bitmap, bool)> {
    // SAFETY: all calls are on live COM objects; each acquired frame is released before the next.
    unsafe {
        let fp16 = hdr_white.and_then(|_| duplicate_fp16(output, device, name));
        let dupl = match fp16 {
            Some(dupl) => dupl,
            None => output.DuplicateOutput(device)?,
        };
        let deadline = Instant::now() + FIRST_FRAME_BUDGET;
        let mut staging: Option<Staging> = None;
        let mut presented = false;
        let mut pointer_visible = false;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource: Option<IDXGIResource> = None;
            match dupl.AcquireNextFrame(remaining.as_millis() as u32, &mut info, &mut resource) {
                Ok(()) => {
                    let copied = copy_to_staging(device, context, resource.as_ref(), &mut staging);
                    dupl.ReleaseFrame()?;
                    copied?;
                    if info.LastPresentTime != 0 {
                        presented = true;
                        pointer_visible = info.PointerPosition.Visible.as_bool();
                        break;
                    }
                }
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => {}
                Err(e) => return Err(e),
            }
            if Instant::now() >= deadline {
                break;
            }
        }
        match staging {
            Some(staging) if presented => Ok((read_frame(context, &staging, hdr_white)?, pointer_visible)),
            _ => Err(DXGI_ERROR_WAIT_TIMEOUT.into()),
        }
    }
}

/// FP16 duplication of an HDR output. `None` when the driver or Windows refuses it, which is logged
/// because the frame is then the 8-bit image Windows makes for SDR, not Laqta's tonemap.
unsafe fn duplicate_fp16(output: &IDXGIOutput1, device: &ID3D11Device, name: &str) -> Option<IDXGIOutputDuplication> {
    let output5 = match output.cast::<IDXGIOutput5>() {
        Ok(output5) => output5,
        Err(e) => {
            tracing::info!("no IDXGIOutput5 for HDR output {name} ({e}); duplicating it in 8 bits");
            return None;
        }
    };
    // SAFETY: a plain call on live COM objects; the format list outlives it.
    match unsafe { output5.DuplicateOutput1(device, 0, &[DXGI_FORMAT_R16G16B16A16_FLOAT]) } {
        Ok(dupl) => Some(dupl),
        Err(e) => {
            tracing::info!("DuplicateOutput1 refused FP16 on HDR output {name} ({e}); duplicating it in 8 bits");
            None
        }
    }
}

unsafe fn copy_to_staging(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    resource: Option<&IDXGIResource>,
    staging: &mut Option<Staging>,
) -> Result<()> {
    // SAFETY: GetDesc writes into a local; CopyResource takes two live textures of equal shape.
    unsafe {
        let texture: ID3D11Texture2D = resource.ok_or_else(|| Error::from(E_FAIL))?.cast()?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        if desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM && desc.Format != DXGI_FORMAT_R16G16B16A16_FLOAT {
            return Err(E_FAIL.into());
        }
        let reusable = staging.as_ref().is_some_and(|s| (s.width, s.height, s.format) == (desc.Width, desc.Height, desc.Format));
        if !reusable {
            let sd = D3D11_TEXTURE2D_DESC {
                Width: desc.Width,
                Height: desc.Height,
                MipLevels: 1,
                ArraySize: 1,
                Format: desc.Format,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };
            let mut created = None;
            device.CreateTexture2D(&sd, None, Some(&mut created))?;
            *staging = Some(Staging {
                texture: created.ok_or_else(|| Error::from(E_FAIL))?,
                width: desc.Width,
                height: desc.Height,
                format: desc.Format,
            });
        }
        if let Some(target) = staging.as_ref() {
            context.CopyResource(&target.texture, &texture);
        }
        Ok(())
    }
}

/// Reads a staging copy into a `Bitmap`. FP16 frames are tonemapped to opaque sRGB; 8-bit frames
/// are made opaque.
fn read_frame(context: &ID3D11DeviceContext, staging: &Staging, hdr_white: Option<f32>) -> Result<Bitmap> {
    let fp16 = staging.format == DXGI_FORMAT_R16G16B16A16_FLOAT;
    let bytes_per_pixel = if fp16 { 8 } else { 4 };
    // SAFETY: `staging.texture` is the texture copied to above and is alive here.
    let raw = unsafe { read_staging(context, &staging.texture, staging.width, staging.height, bytes_per_pixel)? };
    let pixels = if fp16 {
        tonemap_frame(&raw, hdr_white.unwrap_or(FALLBACK_SDR_WHITE))
    } else {
        let mut px = raw;
        for p in px.as_chunks_mut::<4>().0 {
            p[3] = 255;
        }
        px
    };
    Bitmap::new(staging.width, staging.height, PixelFormat::Bgra8, pixels.into()).map_err(|_| E_FAIL.into())
}

/// Copies the packed rows of a mapped staging texture out, `bytes_per_pixel` bytes per pixel.
unsafe fn read_staging(
    context: &ID3D11DeviceContext,
    texture: &ID3D11Texture2D,
    w: u32,
    h: u32,
    bytes_per_pixel: usize,
) -> Result<Vec<u8>> {
    // SAFETY: the mapped pointer is valid for RowPitch * h bytes until Unmap.
    unsafe {
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context.Map(texture, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        let pitch = mapped.RowPitch as usize;
        let row = w as usize * bytes_per_pixel;
        let src = std::slice::from_raw_parts(mapped.pData as *const u8, pitch * h as usize);
        let mut px = Vec::with_capacity(row * h as usize);
        for y in 0..h as usize {
            px.extend_from_slice(&src[y * pitch..y * pitch + row]);
        }
        context.Unmap(texture, 0);
        Ok(px)
    }
}
