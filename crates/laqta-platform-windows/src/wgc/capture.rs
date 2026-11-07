//! Monitor capture for the native recorder: a Windows.Graphics.Capture frame pool on a D3D11 device of the
//! monitor's adapter. The newest frame is held until a tick needs it, then copied to a staging texture and
//! read back as BGRA into the recording's area. Frames that arrive before a tick are dropped.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use laqta_ports::PortError;
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter1, IDXGIDevice, IDXGIFactory1};
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::core::{IInspectable, Interface, factory};

use super::failed;
use super::pure::{Rect, Visible, copy_visible, visible_part};

/// A monitor and the area of it that is recorded: the DXGI adapter and output indexes (as `OsScreen::Windows`
/// names them) and the area in the output's pixels.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Monitor {
    pub(crate) adapter_index: u32,
    pub(crate) output_index: u32,
    pub(crate) area: Rect,
}

/// A running capture of one area. `refresh` brings `latest` up to the newest frame, as BGRA.
pub(crate) struct Capture {
    context: ID3D11DeviceContext,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    _device: ID3D11Device,
    _item: GraphicsCaptureItem,
    closed: Arc<AtomicBool>,
    staging: ID3D11Texture2D,
    area: Rect,
    width: u32,
    height: u32,
    /// The newest frame the pool delivered and that no `refresh` has read yet. It holds a pool buffer until
    /// it is read or replaced.
    pending: Option<Direct3D11CaptureFrame>,
    latest: Vec<u8>,
    has_frame: bool,
}

impl Capture {
    /// Starts capturing `monitor.area` on its monitor. Frames are `area.width` x `area.height` pixels.
    pub(crate) fn start(monitor: Monitor, show_cursor: bool) -> Result<Self, PortError> {
        let Monitor { adapter_index, output_index, area } = monitor;
        if !GraphicsCaptureSession::IsSupported().map_err(failed("Windows.Graphics.Capture"))? {
            return Err(PortError::Unsupported("Windows.Graphics.Capture is not available here; use FFmpeg".into()));
        }
        let (adapter, hmonitor) = find_monitor(adapter_index, output_index)?;
        let (device, context) = create_device(&adapter)?;
        let direct3d = direct3d_device(&device)?;
        let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(failed("capture item factory"))?;
        // SAFETY: the monitor handle is the one DXGI reported for an output enumerated just above.
        let item: GraphicsCaptureItem = unsafe { interop.CreateForMonitor(hmonitor) }.map_err(failed("capture item for the monitor"))?;
        let size = item.Size().map_err(failed("monitor size"))?;
        let target = (u32::try_from(size.Width).unwrap_or(0), u32::try_from(size.Height).unwrap_or(0));
        if visible_part(target, area).is_none() {
            return Err(PortError::Failed("the area is outside its monitor".into()));
        }
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&direct3d, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, size)
            .map_err(failed("frame pool"))?;
        let closed = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&closed);
        item.Closed(&TypedEventHandler::<GraphicsCaptureItem, IInspectable>::new(move |_, _| {
            flag.store(true, Ordering::SeqCst);
            Ok(())
        }))
        .map_err(failed("capture item closed event"))?;
        let session = pool.CreateCaptureSession(&item).map_err(failed("capture session"))?;
        session.SetIsCursorCaptureEnabled(show_cursor).map_err(failed("cursor capture"))?;
        session.StartCapture().map_err(failed("start capture"))?;
        let (width, height) = (area.width, area.height);
        let staging = staging_texture(&device, width, height)?;
        Ok(Capture {
            context,
            pool,
            session,
            _device: device,
            _item: item,
            closed,
            staging,
            area,
            width,
            height,
            pending: None,
            latest: vec![0; width as usize * height as usize * 4],
            has_frame: false,
        })
    }

    /// Takes the newest frame the pool holds and keeps it as pending. Older unread frames are released.
    /// Returns true when a frame arrived since the last call. No pixels are copied here.
    pub(crate) fn poll(&mut self) -> Result<bool, PortError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(PortError::Failed("the captured monitor was closed".into()));
        }
        let mut arrived = false;
        loop {
            match self.pool.TryGetNextFrame() {
                Ok(frame) => {
                    arrived = true;
                    if let Some(older) = self.pending.replace(frame) {
                        let _ = older.Close();
                    }
                }
                // An empty pool comes back as a null frame, which is an error whose code is S_OK.
                Err(error) if error.code().is_ok() => return Ok(arrived),
                Err(error) => return Err(PortError::Failed(format!("capture frame: {error}"))),
            }
        }
    }

    /// Reads the pending frame into `latest`, if there is one. Returns false until the first frame has been
    /// read, so there is nothing to write yet.
    pub(crate) fn refresh(&mut self) -> Result<bool, PortError> {
        if let Some(frame) = self.pending.take() {
            let read = self.read_frame(&frame);
            let _ = frame.Close();
            read?;
            self.has_frame = true;
        }
        Ok(self.has_frame)
    }

    /// The newest frame as BGRA, `width` x `height`. Valid once `refresh` has returned true.
    pub(crate) fn latest(&self) -> &[u8] {
        &self.latest
    }

    /// Ends the capture.
    pub(crate) fn close(mut self) {
        if let Some(frame) = self.pending.take() {
            let _ = frame.Close();
        }
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }

    /// Copies the frame's visible area to the staging texture and reads it back into `latest`. The frame is
    /// still open for both, so the copy is complete before the caller closes it.
    fn read_frame(&mut self, frame: &Direct3D11CaptureFrame) -> Result<(), PortError> {
        let surface = frame.Surface().map_err(failed("frame surface"))?;
        let access: IDirect3DDxgiInterfaceAccess = surface.cast().map_err(failed("frame access"))?;
        // SAFETY: the access object is live, and the texture it returns is a new reference to the frame.
        let texture: ID3D11Texture2D = unsafe { access.GetInterface() }.map_err(failed("frame texture"))?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: GetDesc writes into a local.
        unsafe { texture.GetDesc(&mut desc) };
        let Some(visible) = visible_part((desc.Width, desc.Height), self.area) else {
            self.latest.fill(0);
            return Ok(());
        };
        let source = D3D11_BOX {
            left: visible.src_x,
            top: visible.src_y,
            front: 0,
            right: visible.src_x + visible.width,
            bottom: visible.src_y + visible.height,
            back: 1,
        };
        // SAFETY: both textures are on the capture device, and the box lies inside the source. The destination
        // rectangle lies inside the staging texture, which has the size of the area.
        unsafe {
            self.context.CopySubresourceRegion(&self.staging, 0, visible.dst_x, visible.dst_y, 0, &texture, 0, Some(&raw const source));
        }
        self.read_back(visible)
    }

    fn read_back(&mut self, visible: Visible) -> Result<(), PortError> {
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: the staging texture is CPU readable. Once mapped, it is unmapped below before returning.
        unsafe { self.context.Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }.map_err(failed("map the staging texture"))?;
        let pitch = mapped.RowPitch as usize;
        let len = pitch * self.height as usize;
        // SAFETY: the mapping holds RowPitch bytes for each of the `height` rows until it is unmapped.
        let rows = unsafe { std::slice::from_raw_parts(mapped.pData.cast::<u8>(), len) };
        copy_visible(&mut self.latest, rows, pitch, self.width, self.height, visible);
        // SAFETY: the texture was mapped above.
        unsafe { self.context.Unmap(&self.staging, 0) };
        Ok(())
    }
}

/// The DXGI adapter and the monitor HMONITOR of output `output_index` on adapter `adapter_index`.
fn find_monitor(adapter_index: u32, output_index: u32) -> Result<(IDXGIAdapter1, HMONITOR), PortError> {
    // SAFETY: plain DXGI enumeration; every object is owned by this function.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().map_err(failed("DXGI factory"))?;
        let adapter = factory.EnumAdapters1(adapter_index).map_err(failed("DXGI adapter"))?;
        let output = adapter.EnumOutputs(output_index).map_err(failed("DXGI output"))?;
        let desc = output.GetDesc().map_err(failed("DXGI output description"))?;
        Ok((adapter, desc.Monitor))
    }
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

/// The CPU-readable BGRA texture the frames are copied into before they are read back.
fn staging_texture(device: &ID3D11Device, width: u32, height: u32) -> Result<ID3D11Texture2D, PortError> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut texture = None;
    // SAFETY: the descriptor is a local, and the out-pointer receives a new texture.
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture)) }.map_err(failed("staging texture"))?;
    texture.ok_or_else(|| PortError::Failed("staging texture: none was returned".into()))
}
