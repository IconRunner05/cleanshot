use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HMODULE, LUID, RECT};
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
    DXGI_OUTDUPL_FRAME_INFO, IDXGIAdapter1, IDXGIDevice3, IDXGIFactory1, IDXGIOutput, IDXGIOutput1,
    IDXGIOutputDuplication, IDXGIResource,
};
use windows::core::Interface;

use super::{BgraFrame, Method, MonitorImage, capture_all, size_of_rect};
use crate::Error;

const FIRST_FRAME_TIMEOUT: Duration = Duration::from_millis(500);

/// What the hotkey path copied (D-28), and what each adapter and output's duplication did,
/// for `--log`.
#[derive(Debug)]
pub struct Desktop {
    pub monitors: Vec<MonitorImage>,
    pub copied: Copied,
    /// How long the DXGI factory took to make.
    pub factory: Duration,
    pub adapters: Vec<AdapterReport>,
    pub outputs: Vec<OutputReport>,
}

/// One adapter as DXGI listed it, how long listing it and its outputs took, and where its
/// Direct3D device came from.
#[derive(Debug)]
pub struct AdapterReport {
    /// Its place in DXGI's list, as two adapters can share a name.
    pub index: u32,
    pub adapter: String,
    pub vendor: u32,
    pub listing: Duration,
    pub device: Device,
}

/// Where an adapter's Direct3D device came from for one copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Device {
    /// P-01: the adapter shows no screen, so it got none: a device wakes a sleeping GPU.
    Skipped,
    /// Making one failed; the copy's error says why.
    Failed,
    /// Made for this copy, taking this long.
    Made(Duration),
    /// The device the last copy on this adapter made, kept by [`Devices`].
    Kept,
}

/// P-01: the Direct3D device of each adapter that showed the desktop, kept from one copy to
/// the next so a copy skips making one. A kept device holds no pixels between copies (C-21),
/// and one whose copy failed is not kept, so the next copy makes it afresh.
#[derive(Default)]
pub struct Devices {
    kept: Vec<(LUID, ID3D11Device, ID3D11DeviceContext)>,
}

impl Devices {
    /// P-04: lets every kept device go, with what its driver holds, so the next copy makes its
    /// own. Each is trimmed first, which has its driver drop the buffers it keeps to speed up
    /// the next copy.
    ///
    /// # Panics
    ///
    /// If a kept device is not a DXGI 1.3 device, which Windows 11 rules out.
    pub fn release(&mut self) {
        for (_, device, context) in self.kept.drain(..) {
            let device: IDXGIDevice3 = device
                .cast()
                .expect("a Direct3D 11 device on Windows 11 is a DXGI 1.3 device");
            // SAFETY: plain calls on a device this thread made and its immediate context;
            // Trim asks for ClearState first, so nothing of the device stays bound.
            unsafe {
                context.ClearState();
                device.Trim();
            }
        }
    }

    fn take(&mut self, adapter: LUID) -> Option<(ID3D11Device, ID3D11DeviceContext)> {
        let at = self.kept.iter().position(|(kept, _, _)| *kept == adapter)?;
        let (_, device, context) = self.kept.swap_remove(at);
        Some((device, context))
    }
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

/// Copies every monitor with Desktop Duplication, on the devices `devices` kept from the last
/// copy where it has them, or with GDI `BitBlt` when any output refuses duplication (D-28),
/// before any window of ours is shown (F-02).
///
/// # Errors
///
/// The GDI copy's error when duplication was refused and GDI failed too.
pub fn capture_desktop(devices: &mut Devices) -> Result<Desktop, Error> {
    let Duplicates {
        factory,
        adapters,
        reports: outputs,
        results,
    } = duplicate_all(devices);
    match choose(results) {
        Ok(monitors) => Ok(Desktop {
            monitors,
            copied: Copied::Duplication,
            factory,
            adapters,
            outputs,
        }),
        Err(because) => Ok(Desktop {
            monitors: capture_all(Method::GdiBitBlt)?,
            copied: Copied::Gdi { because },
            factory,
            adapters,
            outputs,
        }),
    }
}

/// Every listed adapter's and output's report, and one result per output, or one error for a
/// factory, adapter or output that fails before it can be duplicated.
#[derive(Default)]
pub(super) struct Duplicates {
    factory: Duration,
    adapters: Vec<AdapterReport>,
    reports: Vec<OutputReport>,
    pub(super) results: Vec<Result<MonitorImage, Error>>,
}

pub(super) fn duplicate_all(devices: &mut Devices) -> Duplicates {
    let mut duplicates = Duplicates::default();
    let making = Instant::now();
    // SAFETY: CreateDXGIFactory1 has no preconditions.
    let factory: Result<IDXGIFactory1, _> = unsafe { CreateDXGIFactory1() };
    duplicates.factory = making.elapsed();
    let factory = match factory {
        Ok(factory) => factory,
        Err(error) => {
            duplicates.results.push(Err(Error::Capture(error)));
            return duplicates;
        }
    };
    for adapter_index in 0.. {
        let listing = Instant::now();
        // SAFETY: an index past the last adapter returns DXGI_ERROR_NOT_FOUND.
        match unsafe { factory.EnumAdapters1(adapter_index) } {
            Ok(adapter) => {
                duplicate_adapter(&adapter, adapter_index, listing, devices, &mut duplicates);
            }
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(error) => {
                duplicates.results.push(Err(Error::Capture(error)));
                break;
            }
        }
    }
    duplicates
}

fn duplicate_adapter(
    adapter: &IDXGIAdapter1,
    index: u32,
    listing: Instant,
    devices: &mut Devices,
    duplicates: &mut Duplicates,
) {
    // SAFETY: a read of the adapter's own description.
    let (name, vendor, luid) = match unsafe { adapter.GetDesc1() } {
        Ok(description) => (
            wide(&description.Description),
            description.VendorId,
            description.AdapterLuid,
        ),
        Err(error) => return duplicates.results.push(Err(Error::Capture(error))),
    };
    let listed = list_outputs(adapter, index, &name, vendor, duplicates);
    let listing = listing.elapsed();
    // P-01: a device wakes a sleeping GPU, so an adapter showing no screen gets none.
    if !listed.iter().any(|(_, report)| report.attached) {
        duplicates.adapters.push(AdapterReport {
            index,
            adapter: name,
            vendor,
            listing,
            device: Device::Skipped,
        });
        return duplicates
            .reports
            .extend(listed.into_iter().map(|(_, report)| report));
    }
    let (device, context, made) = match devices.take(luid) {
        Some((device, context)) => (device, context, Device::Kept),
        None => match make_device(adapter) {
            Ok((device, context, took)) => (device, context, Device::Made(took)),
            Err(error) => {
                duplicates.adapters.push(AdapterReport {
                    index,
                    adapter: name,
                    vendor,
                    listing,
                    device: Device::Failed,
                });
                duplicates
                    .reports
                    .extend(listed.into_iter().map(|(_, report)| report));
                return duplicates.results.push(Err(Error::Capture(error)));
            }
        },
    };
    duplicates.adapters.push(AdapterReport {
        index,
        adapter: name,
        vendor,
        listing,
        device: made,
    });
    let before = duplicates.results.len();
    duplicate_listed(&device, &context, listed, duplicates);
    destroy_released(&context);
    if duplicates.results[before..].iter().all(Result::is_ok) {
        devices.kept.push((luid, device, context));
    }
}

/// C-21: Direct3D 11 defers destroying what is released, so the duplications and staging
/// copies of a capture, which held its pixels, are destroyed here, before the device is kept.
fn destroy_released(context: &ID3D11DeviceContext) {
    // SAFETY: plain calls on the device's own immediate context; nothing of it is bound after.
    unsafe {
        context.ClearState();
        context.Flush();
    }
}

fn make_device(
    adapter: &IDXGIAdapter1,
) -> windows::core::Result<(ID3D11Device, ID3D11DeviceContext, Duration)> {
    let mut device = None;
    let mut context = None;
    let making = Instant::now();
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
    }?;
    let took = making.elapsed();
    Ok((
        device.expect("D3D11CreateDevice returned a device on success"),
        context.expect("D3D11CreateDevice returned a context on success"),
        took,
    ))
}

/// Whether any output of `adapter` is on the desktop.
pub(crate) fn shows_desktop(adapter: &IDXGIAdapter1) -> windows::core::Result<bool> {
    for index in 0.. {
        // SAFETY: an index past the last output returns DXGI_ERROR_NOT_FOUND.
        let output = match unsafe { adapter.EnumOutputs(index) } {
            Ok(output) => output,
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => return Ok(false),
            Err(error) => return Err(error),
        };
        // SAFETY: a read of the output's own description.
        if unsafe { output.GetDesc() }?.AttachedToDesktop.as_bool() {
            return Ok(true);
        }
    }
    unreachable!("the output list ends with DXGI_ERROR_NOT_FOUND")
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
pub(super) fn choose(
    duplicated: Vec<Result<MonitorImage, Error>>,
) -> Result<Vec<MonitorImage>, Error> {
    if duplicated.is_empty() {
        return Err(Error::NothingDuplicated);
    }
    duplicated.into_iter().collect()
}

/// P-01: every attached output's frame is acquired before any is copied, so their waits for
/// the next present overlap, and every copy is queued on the GPU before the first is read.
fn duplicate_listed(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    listed: Vec<(IDXGIOutput, OutputReport)>,
    duplicates: &mut Duplicates,
) {
    let acquired: Vec<_> = listed
        .into_iter()
        .map(|(output, mut report)| {
            let acquired = report.attached.then(|| {
                output
                    .cast::<IDXGIOutput1>()
                    .map_err(Error::Capture)
                    .and_then(|output| acquire(device, &output, report.bounds, &mut report.frame))
            });
            (report, acquired)
        })
        .collect();
    let staged: Vec<_> = acquired
        .into_iter()
        .map(|(report, acquired)| {
            let staged = acquired
                .map(|acquired| acquired.and_then(|acquired| stage(device, context, acquired)));
            (report, staged)
        })
        .collect();
    for (mut report, staged) in staged {
        if let Some(staged) = staged {
            duplicates
                .results
                .push(staged.and_then(|staged| read(context, staged, &mut report.frame)));
        }
        duplicates.reports.push(report);
    }
}

/// The text of a NUL-terminated UTF-16 buffer.
fn wide(buffer: &[u16]) -> String {
    let end = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

/// A duplicated frame still acquired. Dropping it releases it, so every frame acquired is
/// released once, on every path, after its last use.
struct HeldFrame {
    duplication: IDXGIOutputDuplication,
}

impl Drop for HeldFrame {
    fn drop(&mut self) {
        // SAFETY: the duplication holds exactly this one acquired frame.
        let released = unsafe { self.duplication.ReleaseFrame() };
        // Safe to drop: the guard releases each frame once, so the failure is not INVALID_CALL,
        // and whatever else fails, the duplication is released with its capture, which is what
        // ReleaseFrame asks of a lost one.
        debug_assert!(released.is_ok(), "an acquired frame releases");
    }
}

struct Acquired {
    held: HeldFrame,
    texture: ID3D11Texture2D,
    bounds: RECT,
}

struct Staged {
    held: HeldFrame,
    staging: ID3D11Texture2D,
    bounds: RECT,
    copying: Instant,
}

fn acquire(
    device: &ID3D11Device,
    output: &IDXGIOutput1,
    bounds: RECT,
    frame: &mut Option<FrameReport>,
) -> Result<Acquired, Error> {
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
    let held = HeldFrame { duplication };
    frame.replace(FrameReport {
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
    let texture = resource.cast::<ID3D11Texture2D>().map_err(Error::Capture)?;
    Ok(Acquired {
        held,
        texture,
        bounds,
    })
}

/// D-28: `stage` copies upright 8-bit BGRA frames `width` x `height`; GDI copies the rest.
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

/// Whether a duplicated frame was given the desktop. A frame that holds only a pointer update
/// has a texture that was never written, which reads as black.
fn holds_desktop_image(info: &DXGI_OUTDUPL_FRAME_INFO) -> bool {
    info.AccumulatedFrames > 0
}

/// The first frame of a new `duplication` that carries a desktop image, still acquired. A
/// frame with no accumulated desktop update holds only the pointer; its texture was never
/// given the desktop, so it is released and the next is awaited, up to
/// [`FIRST_FRAME_TIMEOUT`] in all.
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

/// Queues a copy of `acquired`'s frame into a new staging texture the CPU can read.
fn stage(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    acquired: Acquired,
) -> Result<Staged, Error> {
    let copying = Instant::now();
    let (width, height) = size_of_rect(acquired.bounds)?;
    let mut description = D3D11_TEXTURE2D_DESC::default();
    // SAFETY: the frame's texture is live and `description` writable.
    unsafe { acquired.texture.GetDesc(&raw mut description) };
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
    // SAFETY: both textures share size and format, and the frame stays acquired until the
    // copy has been read.
    unsafe { context.CopyResource(&staging, &acquired.texture) };
    Ok(Staged {
        held: acquired.held,
        staging,
        bounds: acquired.bounds,
        copying,
    })
}

/// Reads `staged`'s copy into memory, then releases its frame.
fn read(
    context: &ID3D11DeviceContext,
    staged: Staged,
    frame: &mut Option<FrameReport>,
) -> Result<MonitorImage, Error> {
    let (width, height) = size_of_rect(staged.bounds)?;
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    // SAFETY: the map is read only up to each row's width, by RowPitch, and unmapped before
    // the frame is released.
    let pixels = unsafe {
        context
            .Map(&staged.staging, 0, D3D11_MAP_READ, 0, Some(&raw mut mapped))
            .map_err(Error::Capture)?;
        let row = width as usize * 4;
        let pitch = mapped.RowPitch as usize;
        let mut pixels = Vec::with_capacity(row * height as usize);
        for y in 0..height as usize {
            let start = mapped.pData.cast::<u8>().add(y * pitch);
            pixels.extend_from_slice(std::slice::from_raw_parts(start, row));
        }
        context.Unmap(&staged.staging, 0);
        pixels
    };
    drop(staged.held);
    if let Some(report) = frame {
        report.copy = Some(staged.copying.elapsed());
    }
    let frame = BgraFrame::new(width, height, pixels).expect("read takes width x height pixels");
    Ok(MonitorImage {
        bounds: staged.bounds,
        frame,
    })
}

#[cfg(test)]
mod tests {
    use windows::Win32::Graphics::Dxgi::Common::{
        DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_MODE_DESC, DXGI_MODE_ROTATION_ROTATE90,
    };
    use windows::Win32::Graphics::Dxgi::{DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_UNSUPPORTED};

    use super::*;
    use crate::capture::tests::{filled, rect};
    use crate::overlay::tests::screen;
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
    fn d28_a_duplicated_desktop_reports_each_output_and_its_first_frame() {
        let _screen = screen();
        let desktop = capture_desktop(&mut Devices::default()).expect("the desktop is copied");
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
                .any(|adapter| matches!(adapter.device, Device::Made(_))),
            "the duplicating adapter says how long its device took: {:?}",
            desktop.adapters
        );
    }

    #[test]
    fn p01_only_an_adapter_showing_the_desktop_gets_a_device() {
        let _screen = screen();
        let desktop = capture_desktop(&mut Devices::default()).expect("the desktop is copied");
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
                adapter.device != Device::Skipped,
                showing.contains(&adapter.index),
                "{adapter:?}"
            );
        }
    }

    #[test]
    fn p01_a_second_copy_duplicates_on_the_device_the_first_one_made() {
        let _screen = screen();
        let mut devices = Devices::default();
        let first = capture_desktop(&mut devices).expect("the desktop is copied");
        let second = capture_desktop(&mut devices).expect("the desktop is copied again");
        assert!(matches!(second.copied, Copied::Duplication), "{second:?}");
        let made = |desktop: &Desktop, device: fn(Device) -> bool| -> Vec<u32> {
            desktop
                .adapters
                .iter()
                .filter(|adapter| device(adapter.device))
                .map(|adapter| adapter.index)
                .collect()
        };
        let first_made = made(&first, |device| matches!(device, Device::Made(_)));
        assert!(!first_made.is_empty(), "{:?}", first.adapters);
        assert_eq!(
            made(&second, |device| device == Device::Kept),
            first_made,
            "{:?}",
            second.adapters
        );
    }

    #[test]
    fn p04_released_devices_are_let_go_and_the_next_copy_makes_its_own() {
        let _screen = screen();
        let mut devices = Devices::default();
        capture_desktop(&mut devices).expect("the desktop is copied");
        devices.release();
        let again = capture_desktop(&mut devices).expect("the desktop is copied again");
        assert!(
            again
                .adapters
                .iter()
                .all(|adapter| adapter.device != Device::Kept),
            "{:?}",
            again.adapters
        );
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
}
