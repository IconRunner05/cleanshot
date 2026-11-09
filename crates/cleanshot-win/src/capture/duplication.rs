use std::time::{Duration, Instant};

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_STAGING, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_MODE_ROTATION_IDENTITY,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_NOT_FOUND, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_DESC,
    DXGI_OUTDUPL_FRAME_INFO, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput, IDXGIOutput1,
    IDXGIOutputDuplication, IDXGIResource,
};
use windows::core::Interface;

use super::devices::{Device, Devices, Fresh, desktop_luid, make_device, reusable};
use super::{BgraFrame, Method, MonitorImage, capture_all, size_of_rect};
use crate::{Error, text_before_nul};

const FIRST_FRAME_TIMEOUT: Duration = Duration::from_millis(500);

/// What the hotkey path copied, and what each adapter and output's duplication did,
/// for `--log`.
#[derive(Debug)]
pub struct Desktop {
    pub monitors: Vec<MonitorImage>,
    pub copied: Copied,
    /// How long the DXGI factory took to make.
    pub factory: Duration,
    pub adapters: Vec<AdapterReport>,
    pub outputs: Vec<OutputReport>,
    /// How many kept devices were let go because their adapter no longer shows the
    /// desktop.
    pub dropped: usize,
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
/// copy where it has them, or with GDI `BitBlt` when any output refuses duplication,
/// before any window of ours is shown.
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
        dropped,
    } = duplicate_all(devices);
    match choose(results) {
        Ok(monitors) => Ok(Desktop {
            monitors,
            copied: Copied::Duplication,
            factory,
            adapters,
            outputs,
            dropped,
        }),
        Err(because) => Ok(Desktop {
            monitors: capture_all(Method::GdiBitBlt)?,
            copied: Copied::Gdi { because },
            factory,
            adapters,
            outputs,
            dropped,
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
    dropped: usize,
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
    let mut earlier = Devices {
        kept: std::mem::take(&mut devices.kept),
    };
    for adapter_index in 0.. {
        let listing = Instant::now();
        // SAFETY: an index past the last adapter returns DXGI_ERROR_NOT_FOUND.
        match unsafe { factory.EnumAdapters1(adapter_index) } {
            Ok(adapter) => {
                let kept = desktop_luid(&adapter).and_then(|luid| earlier.take(luid));
                duplicate_adapter(
                    &adapter,
                    adapter_index,
                    listing,
                    kept,
                    devices,
                    &mut duplicates,
                );
            }
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(error) => {
                duplicates.results.push(Err(Error::Capture(error)));
                break;
            }
        }
    }
    // What no adapter showing the desktop took is for one that no longer shows it.
    duplicates.dropped = earlier.release();
    duplicates
}

/// Duplicates `adapter`'s outputs on `kept`, the device kept for it, unless that device
/// was removed; and if the kept device cannot duplicate them, once more on a fresh device.
fn duplicate_adapter(
    adapter: &IDXGIAdapter1,
    index: u32,
    listing: Instant,
    kept: Option<(ID3D11Device, ID3D11DeviceContext)>,
    devices: &mut Devices,
    duplicates: &mut Duplicates,
) {
    let marks = (
        duplicates.adapters.len(),
        duplicates.reports.len(),
        duplicates.results.len(),
    );
    let kept = kept.ok_or(Fresh::NoneKept).and_then(|(device, context)| {
        // SAFETY: a read of the device's own state.
        let removed = unsafe { device.GetDeviceRemovedReason() };
        reusable(removed).map(|()| (device, context))
    });
    let used = duplicate_on(adapter, index, listing, kept, devices, duplicates);
    if used == Some(Device::Kept) && duplicates.results[marks.2..].iter().any(Result::is_err) {
        duplicates.adapters.truncate(marks.0);
        duplicates.reports.truncate(marks.1);
        duplicates.results.truncate(marks.2);
        let listing = Instant::now();
        duplicate_on(
            adapter,
            index,
            listing,
            Err(Fresh::Retried),
            devices,
            duplicates,
        );
    }
}

/// Duplicates `adapter`'s outputs on `device`, or on one made for the reason it holds, and
/// keeps that device if every output duplicated; the device it used, or None if it used none.
fn duplicate_on(
    adapter: &IDXGIAdapter1,
    index: u32,
    listing: Instant,
    device: Result<(ID3D11Device, ID3D11DeviceContext), Fresh>,
    devices: &mut Devices,
    duplicates: &mut Duplicates,
) -> Option<Device> {
    // SAFETY: a read of the adapter's own description.
    let (name, vendor, luid) = match unsafe { adapter.GetDesc1() } {
        Ok(description) => (
            text_before_nul(&description.Description),
            description.VendorId,
            description.AdapterLuid,
        ),
        Err(error) => {
            duplicates.results.push(Err(Error::Capture(error)));
            return None;
        }
    };
    let listed = list_outputs(adapter, index, &name, vendor, duplicates);
    let listing = listing.elapsed();
    // A device wakes a sleeping GPU, so an adapter showing no screen gets none.
    if !listed.iter().any(|(_, report)| report.attached) {
        duplicates.adapters.push(AdapterReport {
            index,
            adapter: name,
            vendor,
            listing,
            device: Device::Skipped,
        });
        duplicates
            .reports
            .extend(listed.into_iter().map(|(_, report)| report));
        return None;
    }
    let (device, context, made) = match device {
        Ok((device, context)) => (device, context, Device::Kept),
        Err(because) => match make_device(adapter) {
            Ok((device, context, took)) => (device, context, Device::Made { took, because }),
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
                duplicates.results.push(Err(Error::Capture(error)));
                return Some(Device::Failed);
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
    Some(made)
}

/// Direct3D 11 defers destroying what is released, so the duplications and staging
/// copies of a capture, which held its pixels, are destroyed here, before the device is kept.
fn destroy_released(context: &ID3D11DeviceContext) {
    // SAFETY: plain calls on the device's own immediate context; nothing of it is bound after.
    unsafe {
        context.ClearState();
        context.Flush();
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
                    output: text_before_nul(&description.DeviceName),
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

/// The duplicated frames when every output gave one, otherwise why GDI copies every
/// monitor instead. Frames from two methods are never mixed in one capture.
pub(super) fn choose(
    duplicated: Vec<Result<MonitorImage, Error>>,
) -> Result<Vec<MonitorImage>, Error> {
    if duplicated.is_empty() {
        return Err(Error::NothingDuplicated);
    }
    duplicated.into_iter().collect()
}

/// Every attached output's duplication is made before any frame is acquired, so
/// one present gives each its first frame; every frame is acquired before any is copied, and
/// every copy is queued on the GPU before the first is read.
fn duplicate_listed(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    listed: Vec<(IDXGIOutput, OutputReport)>,
    duplicates: &mut Duplicates,
) {
    let opened: Vec<_> = listed
        .into_iter()
        .map(|(output, report)| {
            let opened = report.attached.then(|| {
                output
                    .cast::<IDXGIOutput1>()
                    .map_err(Error::Capture)
                    .and_then(|output| open(device, &output, report.bounds))
            });
            (report, opened)
        })
        .collect();
    let acquired: Vec<_> = opened
        .into_iter()
        .map(|(mut report, opened)| {
            let acquired =
                opened.map(|opened| opened.and_then(|opened| acquire(opened, &mut report.frame)));
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

/// An output's duplication, made and not yet acquired from.
struct Opened {
    duplication: IDXGIOutputDuplication,
    format: i32,
    duplicate: Duration,
    bounds: RECT,
}

fn open(device: &ID3D11Device, output: &IDXGIOutput1, bounds: RECT) -> Result<Opened, Error> {
    let (width, height) = size_of_rect(bounds)?;
    let duplicating = Instant::now();
    // SAFETY: `device` belongs to this output's adapter.
    let duplication = unsafe { output.DuplicateOutput(device) }.map_err(Error::Capture)?;
    let duplicate = duplicating.elapsed();
    // SAFETY: a read of the duplication's own description.
    let description = unsafe { duplication.GetDesc() };
    copyable(&description, width, height)?;
    Ok(Opened {
        duplication,
        format: description.ModeDesc.Format.0,
        duplicate,
        bounds,
    })
}

fn acquire(opened: Opened, frame: &mut Option<FrameReport>) -> Result<Acquired, Error> {
    let Opened {
        duplication,
        format,
        duplicate,
        bounds,
    } = opened;
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

/// `stage` copies upright 8-bit BGRA frames `width` x `height`; GDI copies the rest.
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
    use windows::Win32::Foundation::LUID;
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
    fn every_output_duplicated_keeps_the_duplicated_frames() {
        let frames = vec![
            Ok(filled(rect(-4, 0, 0, 2), 1)),
            Ok(filled(rect(0, 0, 4, 2), 2)),
        ];
        let kept = choose(frames).expect("every output duplicated");
        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn one_refusing_output_sends_every_monitor_to_gdi() {
        for code in [DXGI_ERROR_UNSUPPORTED, DXGI_ERROR_ACCESS_LOST] {
            let frames = vec![Ok(filled(rect(0, 0, 4, 2), 1)), refused(code)];
            match choose(frames) {
                Err(Error::Capture(error)) => assert_eq!(error.code(), code),
                other => panic!("{code:?} kept duplication: {other:?}"),
            }
        }
    }

    #[test]
    fn no_outputs_sends_every_monitor_to_gdi() {
        assert!(matches!(choose(Vec::new()), Err(Error::NothingDuplicated)));
    }

    #[test]
    fn only_an_upright_8_bit_bgra_frame_the_size_of_its_monitor_is_duplicated() {
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
    fn a_duplicated_desktop_reports_each_output_and_its_first_frame() {
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
                .any(|adapter| matches!(adapter.device, Device::Made { .. })),
            "the duplicating adapter says how long its device took: {:?}",
            desktop.adapters
        );
    }

    #[test]
    fn only_an_adapter_showing_the_desktop_gets_a_device() {
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
    fn a_second_copy_duplicates_on_the_device_the_first_one_made() {
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
        let first_made = made(&first, |device| matches!(device, Device::Made { .. }));
        assert!(!first_made.is_empty(), "{:?}", first.adapters);
        assert_eq!(
            made(&second, |device| device == Device::Kept),
            first_made,
            "{:?}",
            second.adapters
        );
    }

    #[test]
    fn released_devices_are_let_go_and_the_next_copy_makes_its_own() {
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

    /// An adapter that shows the desktop and a device made on another adapter, which cannot
    /// duplicate its outputs; CI's runners list the Basic Render Driver beside it.
    fn showing_and_foreign_device() -> (LUID, (ID3D11Device, ID3D11DeviceContext)) {
        // SAFETY: CreateDXGIFactory1 has no preconditions.
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.expect("a DXGI factory");
        let adapters: Vec<IDXGIAdapter1> = (0..)
            // SAFETY: an index past the last adapter returns DXGI_ERROR_NOT_FOUND.
            .map_while(|index| unsafe { factory.EnumAdapters1(index) }.ok())
            .collect();
        let showing = adapters
            .iter()
            .find_map(desktop_luid)
            .expect("an adapter shows the desktop");
        let other = adapters
            .iter()
            .find(|adapter| desktop_luid(adapter).is_none())
            .expect("this machine lists an adapter that shows nothing");
        let (device, context, _) = make_device(other).expect("a device on the other adapter");
        (showing, (device, context))
    }

    #[test]
    fn the_first_copy_after_the_prewarm_reuses_its_devices() {
        let _screen = screen();
        let mut devices = Devices::default();
        devices.prewarm().expect("the devices are made");
        let desktop = capture_desktop(&mut devices).expect("the desktop is copied");
        for adapter in &desktop.adapters {
            assert!(
                matches!(adapter.device, Device::Kept | Device::Skipped),
                "{:?}",
                desktop.adapters
            );
        }
    }

    #[test]
    fn a_kept_device_that_cannot_duplicate_its_adapter_is_replaced_in_the_same_copy() {
        let _screen = screen();
        let (showing, foreign) = showing_and_foreign_device();
        let mut devices = Devices {
            kept: vec![(showing, foreign.0, foreign.1)],
        };
        let desktop = capture_desktop(&mut devices).expect("the desktop is copied");
        assert!(matches!(desktop.copied, Copied::Duplication), "{desktop:?}");
        assert!(
            desktop.adapters.iter().any(|adapter| matches!(
                adapter.device,
                Device::Made {
                    because: Fresh::Retried,
                    ..
                }
            )),
            "{:?}",
            desktop.adapters
        );
    }

    #[test]
    fn a_device_kept_for_an_adapter_that_no_longer_shows_the_desktop_is_let_go() {
        let _screen = screen();
        let (_, foreign) = showing_and_foreign_device();
        let undocked = LUID {
            LowPart: 0xD151,
            HighPart: 0x7FFF_FFFF,
        };
        let mut devices = Devices {
            kept: vec![(undocked, foreign.0, foreign.1)],
        };
        let desktop = capture_desktop(&mut devices).expect("the desktop is copied");
        assert_eq!(desktop.dropped, 1, "{desktop:?}");
        assert!(
            devices.kept.iter().all(|(luid, _, _)| *luid != undocked),
            "the undocked adapter's device is not kept"
        );
    }

    #[test]
    fn a_pointer_only_frame_is_never_copied() {
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
