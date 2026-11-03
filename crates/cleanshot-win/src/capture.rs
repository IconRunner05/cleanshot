use std::time::{Duration, Instant};

use cleanshot_core::{Monitor, Point, Rect, RgbaImage, first_holding};
pub use windows::Win32::Foundation::RECT;
use windows::Win32::Foundation::{HMODULE, LPARAM, POINT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_MODE_ROTATION_IDENTITY,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_NOT_FOUND, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_DESC,
    DXGI_OUTDUPL_FRAME_INFO, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput, IDXGIOutput1,
    IDXGIOutputDuplication, IDXGIResource,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, EnumDisplayMonitors, GetDC, GetMonitorInfoW, HDC,
    HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect, ReleaseDC, SRCCOPY,
    SelectObject,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::core::{BOOL, Interface};

use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

use crate::Error;
use crate::struct_size;

mod pointer;
mod window;

pub use pointer::draw_pointer;
pub use window::pickable_windows;

pub(crate) const DESKTOP_DPI: f32 = 96.0;
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_millis(500);

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
        Method::GdiBitBlt => monitor_bounds()?.into_iter().map(bitblt).collect(),
        Method::DesktopDuplication => duplicate_all().results.into_iter().collect(),
    }
}

/// F-07: the bounds of the monitor the pointer is on, in virtual-desktop pixels.
///
/// # Errors
///
/// [`Error::Capture`] if Windows cannot say where the pointer is; [`Error::NoMonitorAtPointer`]
/// if no captured monitor holds it.
pub fn monitor_under_cursor(captures: &[MonitorImage]) -> Result<RECT, Error> {
    let mut pointer = POINT::default();
    // SAFETY: `pointer` is writable.
    unsafe { GetCursorPos(&raw mut pointer) }.map_err(Error::Capture)?;
    monitor_containing(captures, pointer).ok_or(Error::NoMonitorAtPointer)
}

fn monitor_containing(captures: &[MonitorImage], point: POINT) -> Option<RECT> {
    first_holding(
        captures.iter().map(|capture| screen_rect(capture.bounds)),
        screen_point(point),
    )
    .map(win32_rect)
}

/// A Win32 rectangle as core's [`Rect`].
#[must_use]
pub fn screen_rect(rect: RECT) -> Rect {
    Rect {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}

pub(crate) fn win32_rect(rect: Rect) -> RECT {
    RECT {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}

pub(crate) fn screen_point(point: POINT) -> Point {
    Point {
        x: point.x,
        y: point.y,
    }
}

/// Physical pixels per DIP on the monitor holding `bounds`.
pub(crate) fn dpi_scale(bounds: RECT) -> windows::core::Result<f32> {
    monitor_scale(nearest_monitor(bounds))
}

/// D-110: the monitor holding most of `bounds`, a capture's rectangle on the virtual desktop,
/// as its work area and scale.
///
/// # Errors
///
/// [`Error::Monitor`] if Windows cannot describe that monitor.
pub fn monitor_of(bounds: RECT) -> Result<Monitor, Error> {
    let monitor = nearest_monitor(bounds);
    let mut info = MONITORINFO {
        cbSize: struct_size::<MONITORINFO>(),
        ..Default::default()
    };
    // SAFETY: `info` has its size set, as GetMonitorInfoW requires.
    if !unsafe { GetMonitorInfoW(monitor, &raw mut info) }.as_bool() {
        return Err(Error::Monitor(windows::core::Error::from_thread()));
    }
    Ok(Monitor {
        work_area: screen_rect(info.rcWork),
        scale: monitor_scale(monitor).map_err(Error::Monitor)?,
    })
}

fn nearest_monitor(bounds: RECT) -> HMONITOR {
    // SAFETY: a read of a local; MONITOR_DEFAULTTONEAREST always returns a monitor.
    unsafe { MonitorFromRect(&raw const bounds, MONITOR_DEFAULTTONEAREST) }
}

#[expect(clippy::cast_precision_loss, reason = "a DPI is a few hundred")]
fn monitor_scale(monitor: HMONITOR) -> windows::core::Result<f32> {
    let (mut x, mut y) = (0, 0);
    // SAFETY: both outputs are writable locals.
    unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &raw mut x, &raw mut y) }?;
    Ok(x as f32 / DESKTOP_DPI)
}

/// What the hotkey path copied (D-28), and what each adapter and output's duplication did,
/// for `--log`.
#[derive(Debug)]
pub struct Desktop {
    pub monitors: Vec<MonitorImage>,
    pub copied: Copied,
    pub adapters: Vec<AdapterReport>,
    pub outputs: Vec<OutputReport>,
}

/// One adapter as DXGI listed it, and how long its Direct3D device took to make; None when
/// no device was made.
#[derive(Debug)]
pub struct AdapterReport {
    /// Its place in DXGI's list, as two adapters can share a name.
    pub index: u32,
    pub adapter: String,
    pub vendor: u32,
    pub device: Option<Duration>,
}

/// How [`Desktop::monitors`] were copied.
#[derive(Debug)]
pub enum Copied {
    Duplication,
    /// Duplication was refused for `because`, so GDI copied every monitor.
    Gdi {
        because: Error,
    },
}

/// One output as DXGI described it, and the first frame its duplication gave, if any.
#[derive(Debug)]
pub struct OutputReport {
    /// The [`AdapterReport::index`] of its adapter.
    pub adapter_index: u32,
    pub adapter: String,
    pub vendor: u32,
    pub output: String,
    pub attached: bool,
    pub bounds: RECT,
    pub rotation: i32,
    pub frame: Option<FrameReport>,
}

/// The facts DXGI gave about the duplicated frame that was copied.
#[derive(Clone, Copy, Debug)]
pub struct FrameReport {
    /// The duplication's `DXGI_FORMAT`.
    pub format: i32,
    /// Frames passed over before it because they held only a pointer update.
    pub pointer_only_frames: u32,
    pub accumulated_frames: u32,
    pub desktop_updated: bool,
    pub pointer_updated: bool,
    pub protected_content_masked: bool,
    /// How long `DuplicateOutput` took.
    pub duplicate: Duration,
    /// How long the wait for this frame took.
    pub acquire: Duration,
    /// How long the frame took to reach memory as RGBA; None when it did not.
    pub copy: Option<Duration>,
}

/// Copies every monitor with Desktop Duplication, or with GDI `BitBlt` when any output
/// refuses duplication (D-28), before any window of ours is shown (F-02).
///
/// # Errors
///
/// The GDI copy's error when duplication was refused and GDI failed too.
pub fn capture_desktop() -> Result<Desktop, Error> {
    let Duplicates {
        adapters,
        reports: outputs,
        results,
    } = duplicate_all();
    match choose(results) {
        Ok(monitors) => Ok(Desktop {
            monitors,
            copied: Copied::Duplication,
            adapters,
            outputs,
        }),
        Err(because) => Ok(Desktop {
            monitors: capture_all(Method::GdiBitBlt)?,
            copied: Copied::Gdi { because },
            adapters,
            outputs,
        }),
    }
}

/// The share of `image`'s pixels whose red, green and blue are all 0.
#[expect(
    clippy::cast_precision_loss,
    reason = "a share for a log line, where a few lost bits do not matter"
)]
#[must_use]
pub fn black_share(image: &RgbaImage) -> f64 {
    let pixels = image.pixels().as_chunks::<4>().0;
    let black = pixels
        .iter()
        .filter(|pixel| pixel[..3] == [0, 0, 0])
        .count();
    black as f64 / pixels.len().max(1) as f64
}

/// The pixels of `selected`, a rectangle on the virtual desktop, cut from `captures`. A
/// part no monitor covers stays transparent.
///
/// # Errors
///
/// [`Error::CaptureShape`] if `selected` is empty.
pub fn region(captures: &[MonitorImage], selected: RECT) -> Result<RgbaImage, Error> {
    let (width, height) = size_of_rect(selected)?;
    let row_bytes = width as usize * 4;
    let mut pixels = vec![0; row_bytes * height as usize];
    for capture in captures {
        let bounds = capture.bounds;
        let Some(overlap) = screen_rect(selected).overlap(screen_rect(bounds)) else {
            continue;
        };
        let source_row_bytes = capture.image.width() as usize * 4;
        let span = overlap.right.abs_diff(overlap.left) as usize * 4;
        for y in overlap.top..overlap.bottom {
            let source = offset(y - bounds.top, overlap.left - bounds.left, source_row_bytes);
            let target = offset(y - selected.top, overlap.left - selected.left, row_bytes);
            pixels[target..target + span]
                .copy_from_slice(&capture.image.pixels()[source..source + span]);
        }
    }
    Ok(RgbaImage::new(width, height, pixels)?)
}

fn offset(row: i32, column: i32, row_bytes: usize) -> usize {
    let row = usize::try_from(row).expect("the row lies inside the overlap");
    let column = usize::try_from(column).expect("the column lies inside the overlap");
    row * row_bytes + column * 4
}

pub(crate) fn monitor_handles() -> Result<Vec<HMONITOR>, Error> {
    unsafe extern "system" fn add(monitor: HMONITOR, _: HDC, _: *mut RECT, list: LPARAM) -> BOOL {
        // SAFETY: `list` is the &mut Vec that monitor_handles passed, alive for the whole call.
        // EnumDisplayMonitors calls this only during our call, with our own LPARAM; no window
        // message carries it, so no other process can supply it.
        // ast-grep-ignore: pointer-from-handle-value
        let list = unsafe { &mut *(list.0 as *mut Vec<HMONITOR>) };
        list.push(monitor);
        true.into()
    }
    let mut handles: Vec<HMONITOR> = Vec::new();
    // SAFETY: `add` only pushes onto `handles`, which outlives the call.
    let listed =
        unsafe { EnumDisplayMonitors(None, None, Some(add), LPARAM(&raw mut handles as isize)) };
    if !listed.as_bool() {
        return Err(Error::Capture(windows::core::Error::from_thread()));
    }
    Ok(handles)
}

fn monitor_bounds() -> Result<Vec<RECT>, Error> {
    monitor_handles()?
        .into_iter()
        .map(|handle| {
            let mut info = MONITORINFO {
                cbSize: struct_size::<MONITORINFO>(),
                ..Default::default()
            };
            // SAFETY: `info` has its size set, as GetMonitorInfoW requires.
            if !unsafe { GetMonitorInfoW(handle, &raw mut info) }.as_bool() {
                return Err(Error::Capture(windows::core::Error::from_thread()));
            }
            Ok(info.rcMonitor)
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
            biSize: struct_size::<BITMAPINFOHEADER>(),
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

/// Every listed adapter's and output's report, and one result per output, or one error for a
/// factory, adapter or output that fails before it can be duplicated.
#[derive(Default)]
struct Duplicates {
    adapters: Vec<AdapterReport>,
    reports: Vec<OutputReport>,
    results: Vec<Result<MonitorImage, Error>>,
}

fn duplicate_all() -> Duplicates {
    let mut duplicates = Duplicates::default();
    // SAFETY: CreateDXGIFactory1 has no preconditions.
    let factory: IDXGIFactory1 = match unsafe { CreateDXGIFactory1() } {
        Ok(factory) => factory,
        Err(error) => {
            duplicates.results.push(Err(Error::Capture(error)));
            return duplicates;
        }
    };
    for adapter_index in 0.. {
        // SAFETY: an index past the last adapter returns DXGI_ERROR_NOT_FOUND.
        match unsafe { factory.EnumAdapters1(adapter_index) } {
            Ok(adapter) => duplicate_adapter(&adapter, adapter_index, &mut duplicates),
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(error) => {
                duplicates.results.push(Err(Error::Capture(error)));
                break;
            }
        }
    }
    duplicates
}

fn duplicate_adapter(adapter: &IDXGIAdapter1, index: u32, duplicates: &mut Duplicates) {
    // SAFETY: a read of the adapter's own description.
    let (name, vendor) = match unsafe { adapter.GetDesc1() } {
        Ok(description) => (wide(&description.Description), description.VendorId),
        Err(error) => return duplicates.results.push(Err(Error::Capture(error))),
    };
    let listed = list_outputs(adapter, index, &name, vendor, duplicates);
    // P-01: a device wakes a sleeping GPU, so an adapter showing no screen gets none.
    if !listed.iter().any(|(_, report)| report.attached) {
        duplicates.adapters.push(AdapterReport {
            index,
            adapter: name,
            vendor,
            device: None,
        });
        return duplicates
            .reports
            .extend(listed.into_iter().map(|(_, report)| report));
    }
    let mut device = None;
    let mut context = None;
    let making = Instant::now();
    // SAFETY: the device is created on `adapter`, as DuplicateOutput requires; outputs are
    // written only on success.
    let created = unsafe {
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
    };
    duplicates.adapters.push(AdapterReport {
        index,
        adapter: name,
        vendor,
        device: created.is_ok().then(|| making.elapsed()),
    });
    if let Err(error) = created {
        duplicates
            .reports
            .extend(listed.into_iter().map(|(_, report)| report));
        return duplicates.results.push(Err(Error::Capture(error)));
    }
    let device: ID3D11Device = device.expect("D3D11CreateDevice returned a device on success");
    let context: ID3D11DeviceContext =
        context.expect("D3D11CreateDevice returned a context on success");
    for (output, report) in listed {
        duplicate_listed(&device, &context, &output, report, duplicates);
    }
}

/// Each output of `adapter` with its report, before any duplication; an output that fails to
/// list or describe itself adds its error to `duplicates` instead.
fn list_outputs(
    adapter: &IDXGIAdapter1,
    index: u32,
    name: &str,
    vendor: u32,
    duplicates: &mut Duplicates,
) -> Vec<(IDXGIOutput, OutputReport)> {
    let mut listed = Vec::new();
    for output_index in 0.. {
        // SAFETY: an index past the last output returns DXGI_ERROR_NOT_FOUND.
        let output = match unsafe { adapter.EnumOutputs(output_index) } {
            Ok(output) => output,
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(error) => {
                duplicates.results.push(Err(Error::Capture(error)));
                break;
            }
        };
        // SAFETY: a read of the output's own description.
        match unsafe { output.GetDesc() } {
            Ok(description) => listed.push((
                output,
                OutputReport {
                    adapter_index: index,
                    adapter: name.to_owned(),
                    vendor,
                    output: wide(&description.DeviceName),
                    attached: description.AttachedToDesktop.as_bool(),
                    bounds: description.DesktopCoordinates,
                    rotation: description.Rotation.0,
                    frame: None,
                },
            )),
            Err(error) => duplicates.results.push(Err(Error::Capture(error))),
        }
    }
    listed
}

/// D-28: the duplicated frames when every output gave one, otherwise why GDI copies every
/// monitor instead. Frames from two methods are never mixed in one capture.
fn choose(duplicated: Vec<Result<MonitorImage, Error>>) -> Result<Vec<MonitorImage>, Error> {
    if duplicated.is_empty() {
        return Err(Error::NothingDuplicated);
    }
    duplicated.into_iter().collect()
}

fn duplicate_listed(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    output: &IDXGIOutput,
    mut report: OutputReport,
    duplicates: &mut Duplicates,
) {
    if !report.attached {
        return duplicates.reports.push(report);
    }
    let image = output
        .cast::<IDXGIOutput1>()
        .map_err(Error::Capture)
        .and_then(|output| {
            duplicate_output(device, context, &output, report.bounds, &mut report.frame)
        });
    duplicates.reports.push(report);
    duplicates.results.push(image);
}

/// The text of a NUL-terminated UTF-16 buffer.
fn wide(buffer: &[u16]) -> String {
    let end = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

fn duplicate_output(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    output: &IDXGIOutput1,
    bounds: RECT,
    frame: &mut Option<FrameReport>,
) -> Result<MonitorImage, Error> {
    let (width, height) = size_of_rect(bounds)?;
    let duplicating = Instant::now();
    // SAFETY: `device` belongs to this output's adapter.
    let duplication = unsafe { output.DuplicateOutput(device) }.map_err(Error::Capture)?;
    let duplicate = duplicating.elapsed();
    // SAFETY: a read of the duplication's own description.
    let description = unsafe { duplication.GetDesc() };
    copyable(&description, width, height)?;
    let format = description.ModeDesc.Format.0;
    let acquiring = Instant::now();
    let (frame_info, resource) = desktop_frame(&duplication)?;
    let acquire = acquiring.elapsed();
    let report = frame.insert(FrameReport {
        format,
        pointer_only_frames: frame_info.pointer_only_frames,
        accumulated_frames: frame_info.info.AccumulatedFrames,
        desktop_updated: frame_info.info.LastPresentTime != 0,
        pointer_updated: frame_info.info.LastMouseUpdateTime != 0,
        protected_content_masked: frame_info.info.ProtectedContentMaskedOut.as_bool(),
        duplicate,
        acquire,
        copy: None,
    });
    let copying = Instant::now();
    let copied = resource
        .cast::<ID3D11Texture2D>()
        .map_err(Error::Capture)
        .and_then(|frame| copy_to_cpu(device, context, &frame, width, height));
    // SAFETY: the frame acquired above is released once, after its last use.
    let released = unsafe { duplication.ReleaseFrame() };
    debug_assert!(released.is_ok(), "an acquired frame releases");
    let mut pixels = copied?;
    bgra_to_rgba(&mut pixels);
    let image = RgbaImage::new(width, height, pixels)?;
    report.copy = Some(copying.elapsed());
    Ok(MonitorImage { bounds, image })
}

/// D-28: `copy_to_cpu` reads upright 8-bit BGRA frames `width` x `height`; GDI copies the rest.
fn copyable(description: &DXGI_OUTDUPL_DESC, width: u32, height: u32) -> Result<(), Error> {
    let mode = &description.ModeDesc;
    if (mode.Format, description.Rotation, mode.Width, mode.Height)
        == (
            DXGI_FORMAT_B8G8R8A8_UNORM,
            DXGI_MODE_ROTATION_IDENTITY,
            width,
            height,
        )
    {
        return Ok(());
    }
    Err(Error::DuplicationNotCopyable {
        format: mode.Format.0,
        rotation: description.Rotation.0,
        width: mode.Width,
        height: mode.Height,
    })
}

struct DesktopFrameInfo {
    info: DXGI_OUTDUPL_FRAME_INFO,
    pointer_only_frames: u32,
}

/// The first frame of a new `duplication` that carries a desktop image, still acquired. A
/// frame with no accumulated desktop update holds only the pointer; its texture was never
/// given the desktop, so it is released and the next is awaited, up to
/// [`FIRST_FRAME_TIMEOUT`] in all.
/// Whether a duplicated frame was given the desktop. A frame that holds only a pointer update
/// has a texture that was never written, which reads as black.
fn holds_desktop_image(info: &DXGI_OUTDUPL_FRAME_INFO) -> bool {
    info.AccumulatedFrames > 0
}

fn desktop_frame(
    duplication: &IDXGIOutputDuplication,
) -> Result<(DesktopFrameInfo, IDXGIResource), Error> {
    let started = Instant::now();
    let mut pointer_only_frames = 0;
    loop {
        let left = FIRST_FRAME_TIMEOUT.saturating_sub(started.elapsed());
        let left = u32::try_from(left.as_millis()).expect("under the 500 ms timeout");
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        // SAFETY: both outputs are writable; an acquired frame is released before the next
        // acquire, or by the caller.
        let acquired =
            unsafe { duplication.AcquireNextFrame(left, &raw mut info, &raw mut resource) };
        match acquired {
            Ok(()) => {}
            Err(error) if error.code() == DXGI_ERROR_WAIT_TIMEOUT => {
                return Err(Error::NoDesktopImage {
                    pointer_only_frames,
                });
            }
            Err(error) => return Err(Error::Capture(error)),
        }
        if holds_desktop_image(&info) {
            let resource = resource.expect("AcquireNextFrame returned a resource on success");
            let frame = DesktopFrameInfo {
                info,
                pointer_only_frames,
            };
            return Ok((frame, resource));
        }
        pointer_only_frames += 1;
        // SAFETY: the pointer-only frame acquired above is released once, unused.
        unsafe { duplication.ReleaseFrame() }.map_err(Error::Capture)?;
    }
}

fn copy_to_cpu(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    frame: &ID3D11Texture2D,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, Error> {
    let mut description = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: `frame` is live and `description` writable.
    unsafe { frame.GetDesc(&raw mut description) };
    // A frame can arrive at a new size mid mode change; reading it as `width` x `height`
    // would run past it.
    if (description.Width, description.Height) != (width, height)
        || description.Format != DXGI_FORMAT_B8G8R8A8_UNORM
    {
        return Err(Error::DuplicationNotCopyable {
            format: description.Format.0,
            rotation: DXGI_MODE_ROTATION_IDENTITY.0,
            width: description.Width,
            height: description.Height,
        });
    }
    description.Usage = D3D11_USAGE_STAGING;
    description.BindFlags = 0;
    description.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0.cast_unsigned();
    description.MiscFlags = 0;
    let mut staging = None;
    // SAFETY: `description` is a valid staging copy of the frame's description.
    unsafe { device.CreateTexture2D(&raw const description, None, Some(&raw mut staging)) }
        .map_err(Error::Capture)?;
    let staging: ID3D11Texture2D = staging.expect("CreateTexture2D returned a texture on success");
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    // SAFETY: both textures share size and format; the map is read only up to each row's
    // width, by RowPitch, and unmapped before return.
    unsafe {
        context.CopyResource(&staging, frame);
        context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&raw mut mapped))
            .map_err(Error::Capture)?;
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

#[cfg(test)]
mod tests {
    use windows::Win32::Graphics::Dxgi::Common::{
        DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_MODE_DESC, DXGI_MODE_ROTATION_ROTATE90,
    };
    use windows::Win32::Graphics::Dxgi::{DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_UNSUPPORTED};

    use super::*;
    use crate::overlay::tests::screen;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    /// A monitor whose pixel n, counted row by row, is `first + n` in every channel, so a
    /// pixel taken from the wrong row or column has the wrong value.
    fn numbered(bounds: RECT, first: u8) -> MonitorImage {
        let (width, height) = size_of_rect(bounds).expect("a test monitor has an area");
        let pixels = (first..)
            .take(width as usize * height as usize)
            .flat_map(|value| [value; 4])
            .collect();
        MonitorImage {
            bounds,
            image: RgbaImage::new(width, height, pixels).expect("sized to match"),
        }
    }

    fn filled(bounds: RECT, value: u8) -> MonitorImage {
        let (width, height) = size_of_rect(bounds).expect("a test monitor has an area");
        let pixels = vec![value; width as usize * height as usize * 4];
        MonitorImage {
            bounds,
            image: RgbaImage::new(width, height, pixels).expect("sized to match"),
        }
    }

    #[test]
    fn f01_a_region_across_a_negative_origin_monitor_takes_each_side_from_its_own_monitor() {
        let captures = [
            numbered(rect(-4, 0, 0, 2), 10),
            numbered(rect(0, 0, 4, 2), 30),
        ];
        let image = region(&captures, rect(-2, 1, 2, 2)).expect("the region has an area");
        assert_eq!((image.width(), image.height()), (4, 1));
        assert_eq!(
            image.pixels(),
            [[16; 4], [17; 4], [34; 4], [35; 4]].concat()
        );
    }

    /// A 3840x2160 monitor at 200% left of a 1920x1080 one at 100%, as a per-monitor-aware
    /// process sees them: both in physical pixels, so neither side is scaled to the other.
    #[test]
    fn f01_a_region_across_a_monitor_above_the_primary_takes_each_side_from_its_own_monitor() {
        let captures = [
            numbered(rect(0, -2, 4, 0), 10),
            numbered(rect(0, 0, 4, 2), 30),
        ];
        let image = region(&captures, rect(1, -1, 3, 1)).expect("the region has an area");
        assert_eq!((image.width(), image.height()), (2, 2));
        assert_eq!(
            image.pixels(),
            [[15; 4], [16; 4], [31; 4], [32; 4]].concat()
        );
        assert_eq!(
            monitor_containing(&captures, POINT { x: 0, y: -1 }),
            Some(rect(0, -2, 4, 0))
        );
    }

    #[test]
    fn d71_a_region_across_a_200_percent_monitor_and_a_100_percent_one_keeps_physical_pixels() {
        let shade =
            |x: i32, y: i32| [u8::try_from((x * 3 + y * 7).rem_euclid(251)).expect("< 251"); 4];
        let painted = |bounds: RECT| {
            let pixels = (bounds.top..bounds.bottom)
                .flat_map(|y| (bounds.left..bounds.right).flat_map(move |x| shade(x, y)))
                .collect();
            let (width, height) = size_of_rect(bounds).expect("a test monitor has an area");
            MonitorImage {
                bounds,
                image: RgbaImage::new(width, height, pixels).expect("sized to match"),
            }
        };
        let captures = [
            painted(rect(-3840, 0, 0, 2160)),
            painted(rect(0, 0, 1920, 1080)),
        ];
        let selected = rect(-2, 1078, 2, 1082);

        let image = region(&captures, selected).expect("the region has an area");

        let expected: Vec<u8> = (selected.top..selected.bottom)
            .flat_map(|y| {
                (selected.left..selected.right).flat_map(move |x| {
                    if x >= 0 && y >= 1080 {
                        [0; 4]
                    } else {
                        shade(x, y)
                    }
                })
            })
            .collect();
        assert_eq!((image.width(), image.height()), (4, 4));
        assert_eq!(image.pixels(), expected);
    }

    #[test]
    fn f01_a_region_no_monitor_covers_stays_transparent() {
        let captures = [filled(rect(0, 0, 2, 2), 30)];
        let image = region(&captures, rect(1, 1, 3, 2)).expect("the region has an area");
        assert_eq!(image.pixels(), [[30; 4], [0; 4]].concat());
    }

    fn refused(code: windows::core::HRESULT) -> Result<MonitorImage, Error> {
        Err(Error::Capture(windows::core::Error::from(code)))
    }

    #[test]
    fn d28_every_output_duplicated_keeps_the_duplicated_frames() {
        let frames = vec![
            Ok(filled(rect(-4, 0, 0, 2), 1)),
            Ok(filled(rect(0, 0, 4, 2), 2)),
        ];
        let kept = choose(frames).expect("every output duplicated");
        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn d28_one_refusing_output_sends_every_monitor_to_gdi() {
        for code in [DXGI_ERROR_UNSUPPORTED, DXGI_ERROR_ACCESS_LOST] {
            let frames = vec![Ok(filled(rect(0, 0, 4, 2), 1)), refused(code)];
            match choose(frames) {
                Err(Error::Capture(error)) => assert_eq!(error.code(), code),
                other => panic!("{code:?} kept duplication: {other:?}"),
            }
        }
    }

    #[test]
    fn d28_no_outputs_sends_every_monitor_to_gdi() {
        assert!(matches!(choose(Vec::new()), Err(Error::NothingDuplicated)));
    }

    #[test]
    fn d28_only_an_upright_8_bit_bgra_frame_the_size_of_its_monitor_is_duplicated() {
        let description = |format, rotation, width, height| DXGI_OUTDUPL_DESC {
            ModeDesc: DXGI_MODE_DESC {
                Width: width,
                Height: height,
                Format: format,
                ..Default::default()
            },
            Rotation: rotation,
            ..Default::default()
        };
        let upright = description(
            DXGI_FORMAT_B8G8R8A8_UNORM,
            DXGI_MODE_ROTATION_IDENTITY,
            1920,
            1080,
        );
        assert!(copyable(&upright, 1920, 1080).is_ok());
        for refused in [
            description(
                DXGI_FORMAT_R16G16B16A16_FLOAT,
                DXGI_MODE_ROTATION_IDENTITY,
                1920,
                1080,
            ),
            description(
                DXGI_FORMAT_B8G8R8A8_UNORM,
                DXGI_MODE_ROTATION_ROTATE90,
                1080,
                1920,
            ),
            description(
                DXGI_FORMAT_B8G8R8A8_UNORM,
                DXGI_MODE_ROTATION_IDENTITY,
                2560,
                1440,
            ),
        ] {
            assert!(
                matches!(
                    copyable(&refused, 1920, 1080),
                    Err(Error::DuplicationNotCopyable { .. })
                ),
                "{:?} {:?}",
                refused.ModeDesc,
                refused.Rotation
            );
        }
    }

    #[test]
    fn f02_black_share_counts_pixels_with_no_red_green_or_blue() {
        let mut pixels = [[0, 0, 0, 255], [0, 0, 1, 255], [7, 0, 0, 255], [0, 0, 0, 0]].concat();
        let image = RgbaImage::new(2, 2, pixels.clone()).expect("2x2 RGBA");
        assert!((black_share(&image) - 0.5).abs() < f64::EPSILON);
        pixels.fill(9);
        let image = RgbaImage::new(2, 2, pixels).expect("2x2 RGBA");
        assert!(black_share(&image).abs() < f64::EPSILON);
    }

    #[test]
    fn d28_a_duplicated_desktop_reports_each_output_and_its_first_frame() {
        let _screen = screen();
        let desktop = capture_desktop().expect("the desktop is copied");
        assert!(matches!(desktop.copied, Copied::Duplication), "{desktop:?}");
        assert_eq!(desktop.outputs.len(), desktop.monitors.len());
        for (output, monitor) in desktop.outputs.iter().zip(&desktop.monitors) {
            assert_eq!(output.bounds, monitor.bounds);
            assert!(
                !output.adapter.is_empty() && !output.output.is_empty(),
                "{output:?}"
            );
            let frame = output.frame.expect("a duplicated output names its frame");
            assert_eq!(frame.format, DXGI_FORMAT_B8G8R8A8_UNORM.0, "{output:?}");
            assert!(
                frame.copy.is_some(),
                "a copied frame says how long: {output:?}"
            );
        }
        assert!(
            desktop
                .adapters
                .iter()
                .any(|adapter| adapter.device.is_some()),
            "the duplicating adapter says how long its device took: {:?}",
            desktop.adapters
        );
    }

    #[test]
    fn p01_only_an_adapter_showing_the_desktop_gets_a_device() {
        let _screen = screen();
        let desktop = capture_desktop().expect("the desktop is copied");
        let showing: Vec<u32> = desktop
            .outputs
            .iter()
            .filter(|output| output.attached)
            .map(|output| output.adapter_index)
            .collect();
        assert!(
            desktop
                .adapters
                .iter()
                .any(|adapter| !showing.contains(&adapter.index)),
            "this machine lists an adapter that shows nothing, such as the Basic Render Driver: {:?}",
            desktop.adapters
        );
        for adapter in &desktop.adapters {
            assert_eq!(
                adapter.device.is_some(),
                showing.contains(&adapter.index),
                "{adapter:?}"
            );
        }
    }

    #[test]
    fn d110_a_capture_names_the_work_area_of_the_monitor_holding_it() {
        for bounds in monitor_bounds().expect("the monitors are listed") {
            let inside = rect(
                bounds.left + 1,
                bounds.top + 1,
                bounds.left + 2,
                bounds.top + 2,
            );
            let monitor = monitor_of(inside).expect("the monitor is described");
            let work = monitor.work_area;
            assert!(
                work.right > work.left && work.bottom > work.top,
                "{monitor:?}"
            );
            assert_eq!(
                screen_rect(bounds).overlap(work),
                Some(work),
                "the work area lies on {bounds:?}"
            );
            assert!(monitor.scale > 0.0, "{monitor:?}");
        }
    }

    #[test]
    fn f01_a_pointer_only_frame_is_never_copied() {
        let pointer_only = DXGI_OUTDUPL_FRAME_INFO {
            LastMouseUpdateTime: 1,
            ..Default::default()
        };
        assert!(!holds_desktop_image(&pointer_only));
        let desktop = DXGI_OUTDUPL_FRAME_INFO {
            LastPresentTime: 1,
            AccumulatedFrames: 1,
            ..Default::default()
        };
        assert!(holds_desktop_image(&desktop));
    }

    #[test]
    fn f07_the_monitor_holding_the_pointer_is_chosen_across_a_negative_origin() {
        let captures = [filled(rect(-4, 0, 0, 2), 1), filled(rect(0, 0, 4, 2), 2)];
        assert_eq!(
            monitor_containing(&captures, POINT { x: -1, y: 1 }),
            Some(rect(-4, 0, 0, 2))
        );
        assert_eq!(
            monitor_containing(&captures, POINT { x: 0, y: 0 }),
            Some(rect(0, 0, 4, 2))
        );
        assert_eq!(monitor_containing(&captures, POINT { x: 4, y: 0 }), None);
    }
}
