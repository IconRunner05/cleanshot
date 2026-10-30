use cleanshot_core::RgbaImage;
use windows::Win32::Foundation::{HMODULE, LPARAM, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_NOT_FOUND, DXGI_OUTDUPL_FRAME_INFO, IDXGIAdapter1,
    IDXGIFactory1, IDXGIOutput1, IDXGIResource,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, EnumDisplayMonitors, GetDC, GetMonitorInfoW, HDC,
    HMONITOR, MONITORINFO, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::core::{BOOL, Interface};

use crate::Error;

const FIRST_FRAME_TIMEOUT_MILLISECONDS: u32 = 500;

/// How the desktop is copied (CHARTER 5 runtime model: the M0 spike picks one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    GdiBitBlt,
    DesktopDuplication,
}

/// One monitor's pixels and where that monitor sits on the virtual desktop, in physical
/// pixels.
#[derive(Debug)]
pub struct MonitorImage {
    pub bounds: RECT,
    pub image: RgbaImage,
}

/// Copies every monitor's pixels with `method`, before any window of ours is shown (F-02).
///
/// # Errors
///
/// [`Error::Capture`] if Windows refuses a step of the copy; [`Error::CaptureShape`] if a
/// monitor reports an empty size.
pub fn capture_all(method: Method) -> Result<Vec<MonitorImage>, Error> {
    match method {
        Method::GdiBitBlt => monitor_bounds().into_iter().map(bitblt).collect(),
        Method::DesktopDuplication => duplicate_all(),
    }
}

pub(crate) fn monitor_handles() -> Vec<HMONITOR> {
    unsafe extern "system" fn add(monitor: HMONITOR, _: HDC, _: *mut RECT, list: LPARAM) -> BOOL {
        // SAFETY: `list` is the &mut Vec that monitor_handles passed, alive for the whole call.
        let list = unsafe { &mut *(list.0 as *mut Vec<HMONITOR>) };
        list.push(monitor);
        true.into()
    }
    let mut handles: Vec<HMONITOR> = Vec::new();
    // SAFETY: `add` only pushes onto `handles`, which outlives the call.
    let listed =
        unsafe { EnumDisplayMonitors(None, None, Some(add), LPARAM(&raw mut handles as isize)) };
    debug_assert!(listed.as_bool(), "the desktop's monitors can be listed");
    handles
}

fn monitor_bounds() -> Vec<RECT> {
    monitor_handles()
        .into_iter()
        .filter_map(|handle| {
            let mut info = MONITORINFO {
                cbSize: u32::try_from(size_of::<MONITORINFO>()).expect("fits in u32"),
                ..Default::default()
            };
            // SAFETY: `info` has its size set, as GetMonitorInfoW requires.
            unsafe { GetMonitorInfoW(handle, &raw mut info) }
                .as_bool()
                .then_some(info.rcMonitor)
        })
        .collect()
}

fn size_of_rect(bounds: RECT) -> Result<(u32, u32), Error> {
    let width = u32::try_from(bounds.right - bounds.left).unwrap_or(0);
    let height = u32::try_from(bounds.bottom - bounds.top).unwrap_or(0);
    if width == 0 || height == 0 {
        return Err(Error::CaptureShape(cleanshot_core::ImageError::ZeroSize {
            width,
            height,
        }));
    }
    Ok((width, height))
}

fn bitblt(bounds: RECT) -> Result<MonitorImage, Error> {
    let (width, height) = size_of_rect(bounds)?;
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: u32::try_from(size_of::<BITMAPINFOHEADER>()).expect("fits in u32"),
            biWidth: bounds.right - bounds.left,
            biHeight: -(bounds.bottom - bounds.top),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let length = width as usize * height as usize * 4;
    // SAFETY: every GDI object is created, used and released here on one thread. The DIB is
    // top-down 32-bit, so `bits` points at width * height * 4 bytes while the bitmap lives,
    // and is copied out before it is deleted.
    unsafe {
        let screen = GetDC(None);
        let memory = CreateCompatibleDC(Some(screen));
        let mut bits = std::ptr::null_mut();
        let copied = CreateDIBSection(
            Some(memory),
            &raw const info,
            DIB_RGB_COLORS,
            &raw mut bits,
            None,
            0,
        )
        .and_then(|bitmap| {
            let previous = SelectObject(memory, bitmap.into());
            let blitted = BitBlt(
                memory,
                0,
                0,
                info.bmiHeader.biWidth,
                -info.bmiHeader.biHeight,
                Some(screen),
                bounds.left,
                bounds.top,
                SRCCOPY | CAPTUREBLT,
            )
            .map(|()| std::slice::from_raw_parts(bits.cast::<u8>(), length).to_vec());
            SelectObject(memory, previous);
            let deleted = DeleteObject(bitmap.into());
            debug_assert!(deleted.as_bool(), "a bitmap made here is deleted once");
            blitted
        });
        let deleted = DeleteDC(memory);
        debug_assert!(deleted.as_bool(), "a memory DC made here is deleted once");
        ReleaseDC(None, screen);
        let mut pixels = copied.map_err(Error::Capture)?;
        bgra_to_rgba(&mut pixels);
        Ok(MonitorImage {
            bounds,
            image: RgbaImage::new(width, height, pixels)?,
        })
    }
}

fn duplicate_all() -> Result<Vec<MonitorImage>, Error> {
    // SAFETY: CreateDXGIFactory1 has no preconditions.
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.map_err(Error::Capture)?;
    let mut images = Vec::new();
    for adapter_index in 0.. {
        // SAFETY: an index past the last adapter returns DXGI_ERROR_NOT_FOUND.
        let adapter = match unsafe { factory.EnumAdapters1(adapter_index) } {
            Ok(adapter) => adapter,
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(error) => return Err(Error::Capture(error)),
        };
        duplicate_adapter(&adapter, &mut images)?;
    }
    Ok(images)
}

fn duplicate_adapter(adapter: &IDXGIAdapter1, images: &mut Vec<MonitorImage>) -> Result<(), Error> {
    let mut device = None;
    let mut context = None;
    // SAFETY: the device is created on `adapter`, as DuplicateOutput requires; outputs are
    // written only on success.
    unsafe {
        D3D11CreateDevice(
            adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&raw mut device),
            None,
            Some(&raw mut context),
        )
    }
    .map_err(Error::Capture)?;
    let device: ID3D11Device = device.expect("D3D11CreateDevice returned a device on success");
    let context: ID3D11DeviceContext =
        context.expect("D3D11CreateDevice returned a context on success");
    for output_index in 0.. {
        // SAFETY: an index past the last output returns DXGI_ERROR_NOT_FOUND.
        let output = match unsafe { adapter.EnumOutputs(output_index) } {
            Ok(output) => output,
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(error) => return Err(Error::Capture(error)),
        };
        let output: IDXGIOutput1 = output.cast().map_err(Error::Capture)?;
        images.push(duplicate_output(&device, &context, &output)?);
    }
    Ok(())
}

fn duplicate_output(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    output: &IDXGIOutput1,
) -> Result<MonitorImage, Error> {
    // SAFETY: `output` and `device` are live and on the same adapter.
    let bounds = unsafe { output.GetDesc() }
        .map_err(Error::Capture)?
        .DesktopCoordinates;
    let (width, height) = size_of_rect(bounds)?;
    // SAFETY: `device` belongs to this output's adapter.
    let duplication = unsafe { output.DuplicateOutput(device) }.map_err(Error::Capture)?;
    let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
    let mut resource: Option<IDXGIResource> = None;
    // SAFETY: both outputs are writable; the frame is released below before returning.
    unsafe {
        duplication.AcquireNextFrame(
            FIRST_FRAME_TIMEOUT_MILLISECONDS,
            &raw mut frame_info,
            &raw mut resource,
        )
    }
    .map_err(Error::Capture)?;
    let copied = resource
        .expect("AcquireNextFrame returned a resource on success")
        .cast::<ID3D11Texture2D>()
        .and_then(|frame| copy_to_cpu(device, context, &frame, width, height));
    // SAFETY: the frame acquired above is released once, after its last use.
    let released = unsafe { duplication.ReleaseFrame() };
    debug_assert!(released.is_ok(), "an acquired frame releases");
    let mut pixels = copied.map_err(Error::Capture)?;
    bgra_to_rgba(&mut pixels);
    Ok(MonitorImage {
        bounds,
        image: RgbaImage::new(width, height, pixels)?,
    })
}

fn copy_to_cpu(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    frame: &ID3D11Texture2D,
    width: u32,
    height: u32,
) -> windows::core::Result<Vec<u8>> {
    let mut description = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: `frame` is live and `description` writable.
    unsafe { frame.GetDesc(&raw mut description) };
    description.Usage = D3D11_USAGE_STAGING;
    description.BindFlags = 0;
    description.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0.cast_unsigned();
    description.MiscFlags = 0;
    let mut staging = None;
    // SAFETY: `description` is a valid staging copy of the frame's description.
    unsafe { device.CreateTexture2D(&raw const description, None, Some(&raw mut staging)) }?;
    let staging: ID3D11Texture2D = staging.expect("CreateTexture2D returned a texture on success");
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    // SAFETY: both textures share size and format; the map is read only up to each row's
    // width, by RowPitch, and unmapped before return.
    unsafe {
        context.CopyResource(&staging, frame);
        context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&raw mut mapped))?;
        let row = width as usize * 4;
        let pitch = mapped.RowPitch as usize;
        let mut pixels = Vec::with_capacity(row * height as usize);
        for y in 0..height as usize {
            let start = mapped.pData.cast::<u8>().add(y * pitch);
            pixels.extend_from_slice(std::slice::from_raw_parts(start, row));
        }
        context.Unmap(&staging, 0);
        Ok(pixels)
    }
}

fn bgra_to_rgba(pixels: &mut [u8]) {
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
        pixel[3] = u8::MAX;
    }
}
