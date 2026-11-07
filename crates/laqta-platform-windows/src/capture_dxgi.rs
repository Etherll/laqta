//! DXGI Desktop Duplication stills. One D3D11 device per adapter, so on hybrid-GPU laptops each
//! output is duplicated on the adapter that owns it. Frames never include the cursor; HDR outputs
//! are read through the 8-bit path (both M1).

use std::time::{Duration, Instant};

use laqta_domain::{Bitmap, Frame, PixelFormat};
use windows::Win32::Foundation::{E_FAIL, HMODULE};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_MODE_ROTATION_IDENTITY, DXGI_MODE_ROTATION_UNSPECIFIED, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput1,
    IDXGIResource,
};
use windows::core::{Error, Interface, Result};

use crate::capture_gdi::monitor_from_handle;

/// How long to wait for a frame whose `LastPresentTime` is set.
const FIRST_FRAME_BUDGET: Duration = Duration::from_millis(500);

type Staging = Option<(ID3D11Texture2D, u32, u32)>;

pub(crate) fn capture() -> Result<Vec<Frame>> {
    // SAFETY: COM calls on interfaces owned by this function; DXGI needs no apartment.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut frames = Vec::new();
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
                let image = duplicate(&output.cast::<IDXGIOutput1>()?, &device, &context)?;
                frames.push(Frame { monitor, image });
            }
        }
        if frames.is_empty() {
            return Err(E_FAIL.into());
        }
        Ok(frames)
    }
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
unsafe fn duplicate(output: &IDXGIOutput1, device: &ID3D11Device, context: &ID3D11DeviceContext) -> Result<Bitmap> {
    // SAFETY: all calls are on live COM objects; each acquired frame is released before the next.
    unsafe {
        let dupl = output.DuplicateOutput(device)?;
        let deadline = Instant::now() + FIRST_FRAME_BUDGET;
        let mut staging: Staging = None;
        let mut presented = false;
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
            Some((texture, w, h)) if presented => read_staging(context, &texture, w, h),
            _ => Err(DXGI_ERROR_WAIT_TIMEOUT.into()),
        }
    }
}

unsafe fn copy_to_staging(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    resource: Option<&IDXGIResource>,
    staging: &mut Staging,
) -> Result<()> {
    // SAFETY: GetDesc writes into a local; CopyResource takes two live textures of equal shape.
    unsafe {
        let texture: ID3D11Texture2D = resource.ok_or_else(|| Error::from(E_FAIL))?.cast()?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        if desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
            return Err(E_FAIL.into());
        }
        if staging.as_ref().is_none_or(|(_, w, h)| (*w, *h) != (desc.Width, desc.Height)) {
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
            *staging = Some((created.ok_or_else(|| Error::from(E_FAIL))?, desc.Width, desc.Height));
        }
        if let Some((target, _, _)) = staging.as_ref() {
            context.CopyResource(target, &texture);
        }
        Ok(())
    }
}

unsafe fn read_staging(context: &ID3D11DeviceContext, texture: &ID3D11Texture2D, w: u32, h: u32) -> Result<Bitmap> {
    // SAFETY: the mapped pointer is valid for RowPitch * h bytes until Unmap.
    unsafe {
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context.Map(texture, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        let pitch = mapped.RowPitch as usize;
        let row = w as usize * 4;
        let src = std::slice::from_raw_parts(mapped.pData as *const u8, pitch * h as usize);
        let mut px = Vec::with_capacity(row * h as usize);
        for y in 0..h as usize {
            px.extend_from_slice(&src[y * pitch..y * pitch + row]);
        }
        context.Unmap(texture, 0);
        for p in px.as_chunks_mut::<4>().0 {
            p[3] = 255;
        }
        Bitmap::new(w, h, PixelFormat::Bgra8, px.into()).map_err(|_| E_FAIL.into())
    }
}
