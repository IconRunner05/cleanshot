use cleanshot_core::RgbaImage;
pub use windows::Win32::Foundation::RECT;
use windows::Win32::Foundation::{HMODULE, LPARAM, POINT};
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

use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

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
        Method::GdiBitBlt => monitor_bounds()?.into_iter().map(bitblt).collect(),
        Method::DesktopDuplication => duplicate_all().into_iter().collect(),
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
    captures
        .iter()
        .map(|capture| capture.bounds)
        .find(|bounds| {
            (bounds.left..bounds.right).contains(&point.x)
                && (bounds.top..bounds.bottom).contains(&point.y)
        })
}

/// What the hotkey path copied (D-28).
#[derive(Debug)]
pub enum Desktop {
    Duplicated(Vec<MonitorImage>),
    /// Duplication was refused for `because`, so GDI copied every monitor.
    CopiedWithGdi {
        monitors: Vec<MonitorImage>,
        because: Error,
    },
}

/// Copies every monitor with Desktop Duplication, or with GDI `BitBlt` when any output
/// refuses duplication (D-28), before any window of ours is shown (F-02).
///
/// # Errors
///
/// The GDI copy's error when duplication was refused and GDI failed too.
pub fn capture_desktop() -> Result<Desktop, Error> {
    match choose(duplicate_all()) {
        Ok(monitors) => Ok(Desktop::Duplicated(monitors)),
        Err(because) => Ok(Desktop::CopiedWithGdi {
            monitors: capture_all(Method::GdiBitBlt)?,
            because,
        }),
    }
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
        let overlap = RECT {
            left: selected.left.max(bounds.left),
            top: selected.top.max(bounds.top),
            right: selected.right.min(bounds.right),
            bottom: selected.bottom.min(bounds.bottom),
        };
        let Ok((overlap_width, _)) = size_of_rect(overlap) else {
            continue;
        };
        let source_row_bytes = capture.image.width() as usize * 4;
        let span = overlap_width as usize * 4;
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
                cbSize: u32::try_from(size_of::<MONITORINFO>()).expect("fits in u32"),
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

/// One result per output, or one error for an adapter or factory that fails before its
/// outputs can be listed.
fn duplicate_all() -> Vec<Result<MonitorImage, Error>> {
    // SAFETY: CreateDXGIFactory1 has no preconditions.
    let factory: IDXGIFactory1 = match unsafe { CreateDXGIFactory1() } {
        Ok(factory) => factory,
        Err(error) => return vec![Err(Error::Capture(error))],
    };
    let mut results = Vec::new();
    for adapter_index in 0.. {
        // SAFETY: an index past the last adapter returns DXGI_ERROR_NOT_FOUND.
        match unsafe { factory.EnumAdapters1(adapter_index) } {
            Ok(adapter) => duplicate_adapter(&adapter, &mut results),
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(error) => {
                results.push(Err(Error::Capture(error)));
                break;
            }
        }
    }
    results
}

fn duplicate_adapter(adapter: &IDXGIAdapter1, results: &mut Vec<Result<MonitorImage, Error>>) {
    let mut device = None;
    let mut context = None;
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
    if let Err(error) = created {
        return results.push(Err(Error::Capture(error)));
    }
    let device: ID3D11Device = device.expect("D3D11CreateDevice returned a device on success");
    let context: ID3D11DeviceContext =
        context.expect("D3D11CreateDevice returned a context on success");
    for output_index in 0.. {
        // SAFETY: an index past the last output returns DXGI_ERROR_NOT_FOUND.
        match unsafe { adapter.EnumOutputs(output_index) } {
            Ok(output) => results.push(
                output
                    .cast::<IDXGIOutput1>()
                    .map_err(Error::Capture)
                    .and_then(|output| duplicate_output(&device, &context, &output)),
            ),
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(error) => {
                results.push(Err(Error::Capture(error)));
                break;
            }
        }
    }
}

/// D-28: the duplicated frames when every output gave one, otherwise why GDI copies every
/// monitor instead. Frames from two methods are never mixed in one capture.
fn choose(duplicated: Vec<Result<MonitorImage, Error>>) -> Result<Vec<MonitorImage>, Error> {
    if duplicated.is_empty() {
        return Err(Error::NothingDuplicated);
    }
    duplicated.into_iter().collect()
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

#[cfg(test)]
mod tests {
    use windows::Win32::Graphics::Dxgi::{DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_UNSUPPORTED};

    use super::*;

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
