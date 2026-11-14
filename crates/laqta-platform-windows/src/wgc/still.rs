//! One still frame of a monitor through Windows.Graphics.Capture. Screenshots use it when DXGI Desktop
//! Duplication fails, or returns a frame that is entirely black (full-screen games, some drivers),
//! before the GDI fallback, which cannot read those surfaces at all. The pointer is never captured.
//! The pure helpers build on every target, so their tests run on Linux.

use laqta_domain::Bitmap;

/// Whether the value of `LAQTA_FORCE_WGC` asks for DXGI duplication to be skipped: it must be
/// exactly `1`.
#[cfg(any(windows, test))]
fn forced(value: Option<&str>) -> bool {
    value == Some("1")
}

/// Whether `LAQTA_FORCE_WGC=1` is set, which turns DXGI duplication off so every screenshot goes
/// through Windows.Graphics.Capture. For testing the fallback.
#[cfg(windows)]
pub(crate) fn forced_by_env() -> bool {
    forced(std::env::var("LAQTA_FORCE_WGC").ok().as_deref())
}

/// Whether the image has pixels and every one of them is black. Alpha is not looked at: a DXGI
/// frame is made opaque, so a blank one is black with alpha 255. The scan stops at the first
/// colour, so an ordinary screenshot costs next to nothing.
#[cfg(any(windows, test))]
pub(crate) fn is_all_black(bitmap: &Bitmap) -> bool {
    let (pixels, _) = bitmap.pixels().as_chunks::<4>();
    !pixels.is_empty() && pixels.iter().all(|pixel| pixel[..3] == [0, 0, 0])
}

#[cfg(windows)]
pub(crate) use imp::{capture_all, capture_monitor};

#[cfg(windows)]
mod imp {
    use std::time::{Duration, Instant};

    use laqta_domain::{Bitmap, Frame, Monitor, PixelFormat};
    use laqta_ports::PortError;
    use windows::Graphics::Capture::{Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession};
    use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
    use windows::Graphics::DirectX::DirectXPixelFormat;
    use windows::Win32::Foundation::{HMODULE, LPARAM, POINT, RECT, TRUE};
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
    use windows::Win32::Graphics::Direct3D11::{
        D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION,
        D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    };
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_SAMPLE_DESC};
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter1, IDXGIDevice, IDXGIFactory1};
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR, MONITOR_DEFAULTTONULL, MonitorFromPoint};
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
    use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
    use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
    use windows::core::{BOOL, Interface, factory};

    use crate::capture_gdi::monitor_from_handle;
    use crate::wgc::failed;

    /// How long to wait for the first frame. Windows.Graphics.Capture delivers one soon after the
    /// capture starts, even when nothing on the screen changes.
    const FIRST_FRAME_BUDGET: Duration = Duration::from_millis(1500);

    /// How often the frame pool is asked for that frame.
    const POLL_INTERVAL: Duration = Duration::from_millis(5);

    /// One frame of every monitor, in the order Windows lists them. Fails when any of them fails, so the
    /// caller can fall back to GDI for all of them.
    pub(crate) fn capture_all() -> Result<Vec<Frame>, PortError> {
        let _apartment = Apartment::enter();
        let mut frames = Vec::new();
        for handle in monitor_handles() {
            let Some(monitor) = monitor_from_handle(handle) else { continue };
            frames.push(grab(handle, monitor, None)?);
        }
        if frames.is_empty() {
            return Err(PortError::Failed("Windows.Graphics.Capture: no monitors found".into()));
        }
        Ok(frames)
    }

    /// One frame of `monitor`, which must still be at the place it was described at. With `sdr_white`
    /// (an HDR monitor), the frame is taken in FP16 and tonemapped like DXGI's; without, Windows makes
    /// the 8-bit image.
    pub(crate) fn capture_monitor(monitor: &Monitor, sdr_white: Option<f32>) -> Result<Frame, PortError> {
        let _apartment = Apartment::enter();
        let centre = POINT {
            x: monitor.rect.x.saturating_add_unsigned(monitor.rect.width / 2),
            y: monitor.rect.y.saturating_add_unsigned(monitor.rect.height / 2),
        };
        // SAFETY: a plain lookup of the monitor at a point; it takes no pointers.
        let handle = unsafe { MonitorFromPoint(centre, MONITOR_DEFAULTTONULL) };
        if handle.0.is_null() {
            return Err(PortError::Failed(format!("Windows.Graphics.Capture: {} is no longer there", monitor.name)));
        }
        match monitor_from_handle(handle) {
            Some(found) if found.name == monitor.name => grab(handle, found, sdr_white),
            _ => Err(PortError::Failed(format!("Windows.Graphics.Capture: {} has moved", monitor.name))),
        }
    }

    /// Captures the first frame of the monitor `handle` and reads it back as an opaque BGRA image.
    fn grab(handle: HMONITOR, monitor: Monitor, sdr_white: Option<f32>) -> Result<Frame, PortError> {
        if !GraphicsCaptureSession::IsSupported().map_err(failed("Windows.Graphics.Capture"))? {
            return Err(PortError::Unsupported("Windows.Graphics.Capture is not available here".into()));
        }
        let adapter = adapter_of(handle)?;
        let (device, context) = create_device(&adapter)?;
        let direct3d = direct3d_device(&device)?;
        let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(failed("capture item factory"))?;
        // SAFETY: the monitor handle is one Windows listed a moment ago.
        let item: GraphicsCaptureItem = unsafe { interop.CreateForMonitor(handle) }.map_err(failed("capture item for the monitor"))?;
        let size = item.Size().map_err(failed("monitor size"))?;
        let format = if sdr_white.is_some() { DirectXPixelFormat::R16G16B16A16Float } else { DirectXPixelFormat::B8G8R8A8UIntNormalized };
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&direct3d, format, 2, size).map_err(failed("frame pool"))?;
        let session = pool.CreateCaptureSession(&item).map_err(failed("capture session"))?;
        let started = session
            .SetIsCursorCaptureEnabled(false)
            .map_err(failed("cursor capture"))
            // Without a border, the screen does not flash a yellow frame. Older Windows has no such
            // setting, and a refusal only leaves the border, which is not part of the image.
            .inspect(|()| {
                let _ = session.SetIsBorderRequired(false);
            })
            .and_then(|()| session.StartCapture().map_err(failed("start capture")));
        let frame = started.and_then(|()| next_frame(&pool)).and_then(|frame| {
            let read = read_frame(&device, &context, &frame, &monitor, sdr_white);
            let _ = frame.Close();
            read
        });
        let _ = session.Close();
        let _ = pool.Close();
        Ok(Frame { image: frame?, monitor })
    }

    /// Waits for the first frame the pool delivers.
    fn next_frame(pool: &Direct3D11CaptureFramePool) -> Result<Direct3D11CaptureFrame, PortError> {
        let deadline = Instant::now() + FIRST_FRAME_BUDGET;
        loop {
            match pool.TryGetNextFrame() {
                Ok(frame) => return Ok(frame),
                // An empty pool comes back as a null frame, which is an error whose code is S_OK.
                Err(error) if error.code().is_ok() => {}
                Err(error) => return Err(PortError::Failed(format!("capture frame: {error}"))),
            }
            if Instant::now() >= deadline {
                return Err(PortError::Failed("Windows.Graphics.Capture delivered no frame".into()));
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    /// Copies the frame to a CPU-readable texture and reads it out. The frame must have the size of the
    /// monitor, or the caller's coordinates would not hold for it.
    fn read_frame(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        frame: &Direct3D11CaptureFrame,
        monitor: &Monitor,
        sdr_white: Option<f32>,
    ) -> Result<Bitmap, PortError> {
        let surface = frame.Surface().map_err(failed("frame surface"))?;
        let access: IDirect3DDxgiInterfaceAccess = surface.cast().map_err(failed("frame access"))?;
        // SAFETY: the access object is live, and the texture it returns is a new reference to the frame.
        let texture: ID3D11Texture2D = unsafe { access.GetInterface() }.map_err(failed("frame texture"))?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: GetDesc writes into a local.
        unsafe { texture.GetDesc(&mut desc) };
        let bytes_per_pixel = match (desc.Format, sdr_white) {
            (DXGI_FORMAT_B8G8R8A8_UNORM, None) => 4,
            (DXGI_FORMAT_R16G16B16A16_FLOAT, Some(_)) => 8,
            _ => return Err(PortError::Failed(format!("Windows.Graphics.Capture: unexpected frame format {}", desc.Format.0))),
        };
        if (desc.Width, desc.Height) != (monitor.rect.width, monitor.rect.height) {
            return Err(PortError::Failed(format!(
                "Windows.Graphics.Capture: the frame is {}x{}, the monitor {}x{}",
                desc.Width, desc.Height, monitor.rect.width, monitor.rect.height
            )));
        }
        let staging = D3D11_TEXTURE2D_DESC {
            MipLevels: 1,
            ArraySize: 1,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
            ..desc
        };
        let mut created = None;
        // SAFETY: the descriptor is a local, and the out-pointer receives a new texture.
        unsafe { device.CreateTexture2D(&staging, None, Some(&mut created)) }.map_err(failed("staging texture"))?;
        let staging = created.ok_or_else(|| PortError::Failed("staging texture: none was returned".into()))?;
        // SAFETY: both textures are on this device and have the same size and format.
        unsafe { context.CopyResource(&staging, &texture) };
        let raw = read_staging(context, &staging, desc.Width, desc.Height, bytes_per_pixel)?;
        let pixels = match sdr_white {
            Some(white) => crate::hdr::tonemap_frame(&raw, white),
            None => {
                let mut raw = raw;
                for pixel in raw.as_chunks_mut::<4>().0 {
                    pixel[3] = 255;
                }
                raw
            }
        };
        Bitmap::new(desc.Width, desc.Height, PixelFormat::Bgra8, pixels.into()).map_err(|error| PortError::Failed(error.to_string()))
    }

    /// The packed rows of a staging texture of `width` x `height` pixels of `bytes_per_pixel` bytes.
    fn read_staging(
        context: &ID3D11DeviceContext,
        texture: &ID3D11Texture2D,
        width: u32,
        height: u32,
        bytes_per_pixel: usize,
    ) -> Result<Vec<u8>, PortError> {
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: the staging texture is CPU readable. Once mapped, it is unmapped below before returning.
        unsafe { context.Map(texture, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }.map_err(failed("map the staging texture"))?;
        let pitch = mapped.RowPitch as usize;
        let row = width as usize * bytes_per_pixel;
        // SAFETY: the mapping holds RowPitch bytes for each of the `height` rows until it is unmapped.
        let source = unsafe { std::slice::from_raw_parts(mapped.pData.cast::<u8>(), pitch * height as usize) };
        let mut pixels = Vec::with_capacity(row * height as usize);
        for y in 0..height as usize {
            pixels.extend_from_slice(&source[y * pitch..y * pitch + row]);
        }
        // SAFETY: the texture was mapped above.
        unsafe { context.Unmap(texture, 0) };
        Ok(pixels)
    }

    /// The DXGI adapter that drives the monitor `handle`, so the capture runs on the GPU that owns it.
    fn adapter_of(handle: HMONITOR) -> Result<IDXGIAdapter1, PortError> {
        // SAFETY: plain DXGI enumeration; every object is owned by this function.
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1().map_err(failed("DXGI factory"))?;
            let mut adapter_index = 0;
            while let Ok(adapter) = factory.EnumAdapters1(adapter_index) {
                adapter_index += 1;
                let mut output_index = 0;
                while let Ok(output) = adapter.EnumOutputs(output_index) {
                    output_index += 1;
                    if output.GetDesc().is_ok_and(|desc| desc.Monitor == handle) {
                        return Ok(adapter);
                    }
                }
            }
        }
        Err(PortError::Failed("Windows.Graphics.Capture: no DXGI output for the monitor".into()))
    }

    /// A D3D11 device on `adapter`, with BGRA support for the frame pool's surfaces.
    fn create_device(adapter: &IDXGIAdapter1) -> Result<(ID3D11Device, ID3D11DeviceContext), PortError> {
        let mut device = None;
        let mut context = None;
        // SAFETY: a real adapter needs D3D_DRIVER_TYPE_UNKNOWN. The out-pointers are locals.
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
            )
        }
        .map_err(failed("D3D11 device"))?;
        match (device, context) {
            (Some(device), Some(context)) => Ok((device, context)),
            _ => Err(PortError::Failed("D3D11 device: none was returned".into())),
        }
    }

    /// The WinRT wrapper of a DXGI device, which the frame pool takes.
    fn direct3d_device(device: &ID3D11Device) -> Result<IDirect3DDevice, PortError> {
        let dxgi: IDXGIDevice = device.cast().map_err(failed("DXGI device"))?;
        // SAFETY: the DXGI device is live for the call.
        let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }.map_err(failed("WinRT Direct3D device"))?;
        inspectable.cast().map_err(failed("WinRT Direct3D device"))
    }

    fn monitor_handles() -> Vec<HMONITOR> {
        unsafe extern "system" fn push(handle: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
            // SAFETY: `data` is the `&mut Vec<HMONITOR>` passed below, alive for the whole call.
            let list = unsafe { &mut *(data.0 as *mut Vec<HMONITOR>) };
            list.push(handle);
            TRUE
        }
        let mut handles: Vec<HMONITOR> = Vec::new();
        // SAFETY: the callback only runs during this call, while `handles` is borrowed.
        let _ = unsafe { EnumDisplayMonitors(None, None, Some(push), LPARAM((&raw mut handles) as isize)) };
        handles
    }

    /// Joins the calling thread to the multithreaded COM apartment, which the WinRT calls need; the
    /// blocking pool's threads have none. It is left again on drop when this call joined it. A
    /// thread that was set up in another mode keeps its setting.
    struct Apartment(bool);

    impl Apartment {
        fn enter() -> Self {
            // SAFETY: plain COM initialization for the calling thread.
            let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            Apartment(hr.is_ok())
        }
    }

    impl Drop for Apartment {
        fn drop(&mut self) {
            if self.0 {
                // SAFETY: this pairs with the successful CoInitializeEx on this thread.
                unsafe { CoUninitialize() };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use laqta_domain::PixelFormat;

    use super::*;

    fn bitmap(format: PixelFormat, pixels: Vec<u8>) -> Bitmap {
        let len = pixels.len() as u32 / 4;
        Bitmap::new(len, 1, format, pixels.into()).expect("a whole number of pixels")
    }

    #[test]
    fn a_blank_frame_is_black_whatever_its_alpha() {
        assert!(is_all_black(&bitmap(PixelFormat::Bgra8, vec![0; 16])));
        assert!(is_all_black(&bitmap(PixelFormat::Bgra8, vec![0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255])));
        assert!(is_all_black(&bitmap(PixelFormat::Rgba8, vec![0, 0, 0, 255])));
    }

    #[test]
    fn one_coloured_pixel_anywhere_makes_it_not_black() {
        let mut pixels = vec![0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255];
        for channel in 0..3 {
            for pixel in [0, 3] {
                let mut image = pixels.clone();
                image[pixel * 4 + channel] = 1;
                assert!(!is_all_black(&bitmap(PixelFormat::Bgra8, image)), "pixel {pixel} channel {channel}");
            }
        }
        pixels[2 * 4] = 255;
        assert!(!is_all_black(&bitmap(PixelFormat::Rgba8, pixels)));
    }

    #[test]
    fn an_image_without_pixels_is_not_black() {
        assert!(!is_all_black(&Bitmap::new(0, 0, PixelFormat::Bgra8, Vec::new().into()).expect("an empty bitmap")));
    }

    #[test]
    fn only_one_turns_the_fallback_on() {
        assert!(forced(Some("1")));
        assert!(!forced(None));
        assert!(!forced(Some("")));
        assert!(!forced(Some("0")));
        assert!(!forced(Some("true")));
    }
}
