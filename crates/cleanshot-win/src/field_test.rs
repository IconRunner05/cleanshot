use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_INVALID_DATA, HMODULE, LPARAM, RECT, WPARAM,
};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_PIXEL_GEOMETRY, DWRITE_RENDERING_MODE, DWriteCreateFactory,
    IDWriteFactory, IDWriteRenderingParams1,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_ERROR_NOT_FOUND, IDXGIFactory1,
};
use windows::Win32::Graphics::Gdi::{
    DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW, GetMonitorInfoW, HMONITOR, MONITORINFO,
    MONITORINFOEXW,
};
use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;
use windows::Win32::System::Ole::{CF_DIBV5, CF_UNICODETEXT};
use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    VIRTUAL_KEY, VK_CONTROL, VK_E, VK_ESCAPE, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, FindWindowW, GetForegroundWindow, IsWindow, MSG, MsgWaitForMultipleObjects,
    PM_REMOVE, PeekMessageW, PostMessageW, QS_ALLINPUT, TranslateMessage, WM_CLOSE,
};
use windows::core::{Interface, PCWSTR, w};

pub use crate::fade::animations_on;
use crate::overlay::{Overlay, WINDOW_CLASS};

use crate::Error;
use crate::capture::{desktop_adapters, make_device, monitor_handles};
use crate::registry::{read_number, read_text};
use crate::struct_size;
use cleanshot_core::{Hotkey, HotkeyAction};

use crate::tray::{Request, Tray, running_tray_window};

pub mod dpi;

/// The Windows release, read from the registry.
#[derive(Debug)]
pub struct WindowsVersion {
    pub display_version: String,
    pub build: u32,
    pub revision: u32,
    pub edition: String,
    pub installation_type: String,
}

/// One monitor as Windows reports it to a per-monitor-DPI-aware process.
#[derive(Debug)]
pub struct Monitor {
    pub device: String,
    pub bounds: RECT,
    pub dpi: Result<u32, windows::core::Error>,
    /// Refresh rate in hertz, or None if Windows does not report the current mode.
    pub refresh_hertz: Option<u32>,
}

/// One DXGI adapter.
#[derive(Debug)]
pub struct Adapter {
    pub description: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub software: bool,
    pub dedicated_video_memory: usize,
}

/// What the tray and its hotkey did when driven by synthetic input.
#[derive(Debug)]
pub struct TrayRun {
    pub icon_shown: bool,
    pub hotkey: (Hotkey, Result<(), String>),
    pub foreground_window: bool,
    pub inputs_sent: u32,
    pub hotkey_delivered_after: Option<Duration>,
}

/// The rendering params DirectWrite gives a process that asks for the system's, the values
/// GPUI reads for its text gamma and contrast (`directx_renderer.rs:855-862` at 14b7b0e).
#[derive(Debug)]
pub struct RenderingParams {
    pub gamma: f32,
    pub enhanced_contrast: f32,
    pub grayscale_enhanced_contrast: f32,
    pub cleartype_level: f32,
    pub pixel_geometry: DWRITE_PIXEL_GEOMETRY,
    pub rendering_mode: DWRITE_RENDERING_MODE,
}

/// F-34: what `IDWriteFactory::CreateRenderingParams` returns on this machine.
///
/// # Errors
///
/// The Windows error if DirectWrite refuses the factory or the params.
pub fn rendering_params() -> Result<RenderingParams, windows::core::Error> {
    // SAFETY: the factory and params are COM objects owned here; each getter reads a value.
    unsafe {
        let factory: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
        let params = factory.CreateRenderingParams()?;
        let params1: IDWriteRenderingParams1 = params.cast()?;
        Ok(RenderingParams {
            gamma: params.GetGamma(),
            enhanced_contrast: params.GetEnhancedContrast(),
            grayscale_enhanced_contrast: params1.GetGrayscaleEnhancedContrast(),
            cleartype_level: params.GetClearTypeLevel(),
            pixel_geometry: params.GetPixelGeometry(),
            rendering_mode: params.GetRenderingMode(),
        })
    }
}

/// Reads the Windows release from `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion`.
///
/// # Errors
///
/// The Windows error of the first value that cannot be read.
pub fn windows_version() -> Result<WindowsVersion, windows::core::Error> {
    Ok(WindowsVersion {
        display_version: registry_string(w!("DisplayVersion"))?,
        build: registry_string(w!("CurrentBuildNumber"))?
            .parse()
            .map_err(|_| windows::core::Error::from(ERROR_INVALID_DATA))?,
        revision: registry_number(w!("UBR"))?,
        edition: registry_string(w!("EditionID"))?,
        installation_type: registry_string(w!("InstallationType"))?,
    })
}

impl WindowsVersion {
    /// The release's name, from its build: the registry's `ProductName` says Windows 10 on
    /// Windows 11 too, so it is not read.
    #[must_use]
    pub fn name(&self) -> &'static str {
        if self.build >= FIRST_WINDOWS_11_BUILD {
            "Windows 11"
        } else {
            "Windows 10"
        }
    }
}

const FIRST_WINDOWS_11_BUILD: u32 = 22000;

const CURRENT_VERSION: PCWSTR = w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion");

fn registry_string(value: PCWSTR) -> Result<String, windows::core::Error> {
    read_text(HKEY_LOCAL_MACHINE, CURRENT_VERSION, value)?
        .ok_or_else(|| ERROR_FILE_NOT_FOUND.into())
}

fn registry_number(value: PCWSTR) -> Result<u32, windows::core::Error> {
    read_number(HKEY_LOCAL_MACHINE, CURRENT_VERSION, value)?
        .ok_or_else(|| ERROR_FILE_NOT_FOUND.into())
}

/// Every monitor attached to the desktop, in the order Windows lists them.
///
/// # Errors
///
/// [`Error::Capture`] if Windows cannot list the monitors.
pub fn monitors() -> Result<Vec<Monitor>, Error> {
    Ok(monitor_handles()?
        .into_iter()
        .map(describe_monitor)
        .collect())
}

fn describe_monitor(handle: HMONITOR) -> Monitor {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = struct_size::<MONITORINFOEXW>();
    // SAFETY: `info` is a MONITORINFOEXW with its size set, as GetMonitorInfoW requires.
    let described = unsafe { GetMonitorInfoW(handle, (&raw mut info).cast::<MONITORINFO>()) };
    let length = info
        .szDevice
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(info.szDevice.len());
    let (mut dpi_x, mut dpi_y) = (0, 0);
    // SAFETY: `handle` came from EnumDisplayMonitors, and both outputs are writable.
    let dpi =
        unsafe { GetDpiForMonitor(handle, MDT_EFFECTIVE_DPI, &raw mut dpi_x, &raw mut dpi_y) }
            .map(|()| dpi_x);
    let mut mode = DEVMODEW {
        dmSize: u16::try_from(size_of::<DEVMODEW>()).expect("fits in u16"),
        ..Default::default()
    };
    // SAFETY: szDevice is the NUL-terminated device name GetMonitorInfoW wrote, and `mode`
    // has its size set.
    let listed = described.as_bool()
        && unsafe {
            EnumDisplaySettingsW(
                PCWSTR(info.szDevice.as_ptr()),
                ENUM_CURRENT_SETTINGS,
                &raw mut mode,
            )
        }
        .as_bool();
    Monitor {
        device: if described.as_bool() {
            String::from_utf16_lossy(&info.szDevice[..length])
        } else {
            String::from("(GetMonitorInfoW failed)")
        },
        bounds: info.monitorInfo.rcMonitor,
        dpi,
        refresh_hertz: listed.then_some(mode.dmDisplayFrequency),
    }
}

/// Every DXGI adapter, hardware and software.
///
/// # Errors
///
/// The Windows error if the DXGI factory or an adapter description cannot be read.
pub fn adapters() -> Result<Vec<Adapter>, windows::core::Error> {
    // SAFETY: CreateDXGIFactory1 has no preconditions.
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }?;
    let mut found = Vec::new();
    for index in 0.. {
        // SAFETY: `factory` is a live DXGI factory; an index past the last adapter returns
        // DXGI_ERROR_NOT_FOUND.
        let adapter = match unsafe { factory.EnumAdapters1(index) } {
            Ok(adapter) => adapter,
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(error) => return Err(error),
        };
        // SAFETY: `adapter` is live.
        let description = unsafe { adapter.GetDesc1() }?;
        let length = description
            .Description
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(description.Description.len());
        found.push(Adapter {
            description: String::from_utf16_lossy(&description.Description[..length]),
            vendor_id: description.VendorId,
            device_id: description.DeviceId,
            software: description.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0.cast_unsigned() != 0,
            dedicated_video_memory: description.DedicatedVideoMemory,
        });
    }
    Ok(found)
}

/// A Direct3D 11 driver the probe tries.
#[derive(Clone, Copy, Debug)]
pub enum Driver {
    Hardware,
    Warp,
}

/// The feature level a D3D11 device gets on `driver`, as Direct3D's `D3D_FEATURE_LEVEL` value
/// (for example `0xb100` for level `11_1`).
///
/// # Errors
///
/// The Windows error if no device can be created on that driver.
pub fn d3d11_feature_level(driver: Driver) -> Result<u32, windows::core::Error> {
    let driver_type: D3D_DRIVER_TYPE = match driver {
        Driver::Hardware => D3D_DRIVER_TYPE_HARDWARE,
        Driver::Warp => D3D_DRIVER_TYPE_WARP,
    };
    let mut level = D3D_FEATURE_LEVEL::default();
    // SAFETY: no adapter or software module is passed, and the device is dropped at once.
    unsafe {
        D3D11CreateDevice(
            None,
            driver_type,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            None,
            Some(&raw mut level),
            None,
        )
    }?;
    Ok(level.0.cast_unsigned())
}

/// What making the capture's Direct3D device costs on one adapter that shows the desktop, for
/// pricing one made at start-up (P-01, P-03, P-04).
#[derive(Debug)]
pub struct DeviceCost {
    /// The adapter's place in DXGI's list.
    pub adapter_index: u32,
    pub made: Duration,
    /// Working set and private working set bytes while the device is held, less those before.
    pub held: (isize, isize),
    /// The same once the device is dropped again.
    pub dropped: (isize, isize),
}

/// Makes and drops the capture's device on each adapter with an output on the desktop, one at
/// a time, measuring each.
///
/// # Errors
///
/// The Windows error if DXGI cannot list the adapters, a device cannot be made or the
/// process's memory counters cannot be read.
pub fn capture_device_costs() -> windows::core::Result<Vec<DeviceCost>> {
    let mut costs = Vec::new();
    for (adapter_index, adapter) in desktop_adapters()? {
        let before = held_bytes()?;
        let (device, context, made) = make_device(&adapter)?;
        let held = held_bytes()?;
        drop((device, context));
        let dropped = held_bytes()?;
        costs.push(DeviceCost {
            adapter_index,
            made,
            held: (held.0 - before.0, held.1 - before.1),
            dropped: (dropped.0 - before.0, dropped.1 - before.1),
        });
    }
    Ok(costs)
}

/// This process's working set and private working set, in bytes.
/// One part of [`Overlay::prewarm`], or the release of its Direct2D factory after it.
#[derive(Debug)]
pub struct PrewarmPart {
    pub part: &'static str,
    pub took: Duration,
    /// Private working set bytes after the part, less those before it.
    pub private_bytes: isize,
}

/// Why [`prewarm_costs`] could not price the prewarm.
#[derive(Debug, thiserror::Error)]
pub enum PrewarmCostError {
    #[error("the prewarm failed")]
    Prewarm(#[source] Error),
    #[error("this process's memory could not be read")]
    Memory(#[source] windows::core::Error),
}

/// P-01 against P-04: runs the overlay prewarm on this thread and then lets go of its Direct2D
/// factory, as `GRAPHICS_IDLE` does, timing each part and the private working set it adds.
/// Only the first call in a process sees Direct2D and DirectWrite start.
///
/// # Errors
///
/// [`PrewarmCostError`] if the prewarm fails or the memory counters cannot be read.
pub fn prewarm_costs() -> Result<Vec<PrewarmPart>, PrewarmCostError> {
    let mut marks = vec![("start", Instant::now(), held_bytes())];
    Overlay::prewarm_marked(|part| marks.push((part, Instant::now(), held_bytes())))
        .map_err(PrewarmCostError::Prewarm)?;
    crate::render_target::release_factory();
    marks.push(("factory_released", Instant::now(), held_bytes()));
    marks
        .iter()
        .zip(marks.iter().skip(1))
        .map(|((_, then, before), (part, now, after))| {
            let private = |held: &windows::core::Result<(isize, isize)>| {
                held.as_ref()
                    .map(|&(_, private)| private)
                    .map_err(|error| PrewarmCostError::Memory(error.clone()))
            };
            Ok(PrewarmPart {
                part,
                took: *now - *then,
                private_bytes: private(after)? - private(before)?,
            })
        })
        .collect()
}

fn held_bytes() -> windows::core::Result<(isize, isize)> {
    let counters = crate::log::memory_counters()?;
    let signed = |bytes: usize| isize::try_from(bytes).expect("a working set fits in isize");
    Ok((
        signed(counters.WorkingSetSize),
        signed(counters.PrivateWorkingSetSize),
    ))
}

/// Creates the tray, presses Ctrl+Shift+4 with `SendInput`, and pumps this thread's messages
/// until the tray reports the hotkey or `timeout` passes.
///
/// # Panics
///
/// Never: the probe's hotkey text is fixed and valid.
///
/// # Errors
///
/// [`Error::Window`] if the tray window cannot be created.
pub fn drive_tray(timeout: Duration) -> Result<TrayRun, Error> {
    let delivered = Rc::new(Cell::new(false));
    let seen = Rc::clone(&delivered);
    let region = Hotkey::parse("Ctrl+Shift+4").expect("Ctrl+Shift+4 is a valid hotkey");
    let tray = Tray::create(
        &[(Request::Capture(HotkeyAction::Region), region)],
        move |request| {
            if request == Request::Capture(HotkeyAction::Region) {
                seen.set(true);
            }
        },
    )?;
    // SAFETY: GetForegroundWindow has no preconditions.
    let foreground_window = !unsafe { GetForegroundWindow() }.is_invalid();
    let inputs_sent = press(&[VK_CONTROL, VK_SHIFT, VIRTUAL_KEY(u16::from(b'4'))]);
    let start = Instant::now();
    while !delivered.get() && start.elapsed() < timeout {
        pump_messages(timeout.saturating_sub(start.elapsed()));
    }
    Ok(TrayRun {
        icon_shown: tray.icon_shown(),
        hotkey: {
            let (_, hotkey, registered) = tray
                .hotkeys()
                .next()
                .expect("the tray holds the one hotkey it was given");
            (hotkey, registered.map_err(ToString::to_string))
        },
        foreground_window,
        inputs_sent,
        hotkey_delivered_after: delivered.get().then(|| start.elapsed()),
    })
}

/// Why [`exit_running_tray`] failed.
#[derive(Debug, thiserror::Error)]
pub enum ExitRunError {
    #[error("no Cleanshot tray window is open")]
    NoTray(#[source] windows::core::Error),
    #[error("Windows refused the message asking Cleanshot to exit")]
    NotAsked(#[source] windows::core::Error),
    #[error("the tray window was still open {0:?} after Cleanshot was asked to exit")]
    StillOpen(Duration),
    #[error("Cleanshot exited without logging that the shell removed its tray icon")]
    IconRemovalNotLogged,
}

/// Asks the Cleanshot running in this session to exit, as its menu's Exit does, waits up to
/// `timeout` for its tray window to close, and then asks `removed` whether Cleanshot logged that
/// the shell accepted the removal of its tray icon (F-50). Another process cannot tell whether
/// the shell still draws an icon whose window is gone, so the log is the proof.
///
/// # Errors
///
/// [`ExitRunError`] for the first step that did not happen.
pub fn exit_running_tray(
    timeout: Duration,
    removed: impl FnOnce() -> bool,
) -> Result<(), ExitRunError> {
    let window = running_tray_window().map_err(ExitRunError::NoTray)?;
    // SAFETY: posting copies no memory; a window that closes first fails the post.
    unsafe { PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0)) }
        .map_err(ExitRunError::NotAsked)?;
    // SAFETY: IsWindow only reads whether the handle still names a window.
    if !within(timeout, || !unsafe { IsWindow(Some(window)) }.as_bool()) {
        return Err(ExitRunError::StillOpen(timeout));
    }
    if !removed() {
        return Err(ExitRunError::IconRemovalNotLogged);
    }
    Ok(())
}

/// Why [`open_and_cancel_overlays`] stopped, naming the press it stopped on, from 1.
#[derive(Debug, thiserror::Error)]
pub enum OverlayRunError {
    #[error("Windows refused the key events of press {0}")]
    KeysRefused(u32),
    #[error("press {0} showed no overlay in time")]
    NotShown(u32),
    #[error("the overlay of press {0} was still open after Esc")]
    NotClosed(u32),
}

/// Presses Ctrl+Shift+4 and then Esc `times` times, for another process's overlay to open and
/// close (P-01). Each press waits up to `timeout` for `shown(press)` to hold and then for every
/// overlay window to close, so no press lands while the last overlay is still up, and then
/// `settle` more, as a user's next capture would.
///
/// # Errors
///
/// [`OverlayRunError`] for the first press that Windows refused or that did not show or
/// close its overlay within `timeout`.
pub fn open_and_cancel_overlays(
    times: u32,
    timeout: Duration,
    settle: Duration,
    mut shown: impl FnMut(u32) -> bool,
) -> Result<(), OverlayRunError> {
    let region = [VK_CONTROL, VK_SHIFT, VIRTUAL_KEY(u16::from(b'4'))];
    for number in 1..=times {
        if press(&region) != 6 {
            return Err(OverlayRunError::KeysRefused(number));
        }
        if !within(timeout, || shown(number)) {
            return Err(OverlayRunError::NotShown(number));
        }
        if press(&[VK_ESCAPE]) != 2 {
            return Err(OverlayRunError::KeysRefused(number));
        }
        if !within(timeout, || !overlay_open()) {
            return Err(OverlayRunError::NotClosed(number));
        }
        std::thread::sleep(settle);
    }
    Ok(())
}

/// Why [`edit_and_close`] stopped.
#[derive(Debug, thiserror::Error)]
pub enum EditorRunError {
    #[error("Windows refused the key events of {0}")]
    KeysRefused(&'static str),
    #[error("the full-screen press showed no overlay in time")]
    OverlayNotShown,
    #[error("the editor drew no first frame in time")]
    NotDrawn,
    #[error("no editor window was found to close: {0}")]
    NotFound(windows::core::Error),
    #[error("the editor could not be asked to close: {0}")]
    NotClosed(windows::core::Error),
}

/// P-04 after the editor: presses [`FULL_SCREEN`] for another process's overlay over the whole
/// monitor, then E to edit it, and once `drawn` holds closes the editor as Alt+F4 does. Waits up
/// to `timeout` each for `shown` and `drawn`.
///
/// # Errors
///
/// [`EditorRunError`] for the first step that Windows refused or that did not happen in time.
pub fn edit_and_close(
    timeout: Duration,
    shown: impl FnMut() -> bool,
    drawn: impl FnMut() -> bool,
) -> Result<(), EditorRunError> {
    if press(&FULL_SCREEN) != 6 {
        return Err(EditorRunError::KeysRefused("Ctrl+Shift+7"));
    }
    if !within(timeout, shown) {
        return Err(EditorRunError::OverlayNotShown);
    }
    if press(&[VK_E]) != 2 {
        return Err(EditorRunError::KeysRefused("E"));
    }
    if !within(timeout, drawn) {
        return Err(EditorRunError::NotDrawn);
    }
    // D-110: GPUI's window class and the editor's fixed title, so no window's own title is read.
    // SAFETY: a lookup by two 'static strings; no window is touched.
    let editor = unsafe { FindWindowW(w!("Zed::Window"), w!("Cleanshot")) }
        .map_err(EditorRunError::NotFound)?;
    // WM_CLOSE, not Alt+F4 keys: the keys go to whatever is in front, which may be this console.
    // SAFETY: WM_CLOSE carries no pointers.
    unsafe { PostMessageW(Some(editor), WM_CLOSE, WPARAM(0), LPARAM(0)) }
        .map_err(EditorRunError::NotClosed)
}

fn within(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while !done() {
        if start.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    true
}

fn overlay_open() -> bool {
    // SAFETY: a lookup by a 'static class name; no window is touched.
    unsafe { FindWindowW(WINDOW_CLASS, PCWSTR::null()) }.is_ok()
}

fn press(keys: &[VIRTUAL_KEY]) -> u32 {
    let event = |key: VIRTUAL_KEY, flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                dwFlags: flags,
                ..Default::default()
            },
        },
    };
    let inputs: Vec<INPUT> = keys
        .iter()
        .map(|&key| event(key, KEYBD_EVENT_FLAGS(0)))
        .chain(keys.iter().rev().map(|&key| event(key, KEYEVENTF_KEYUP)))
        .collect();
    let size = i32::try_from(size_of::<INPUT>()).expect("INPUT fits in i32");
    // SAFETY: `inputs` holds only keyboard events, each of the size passed.
    unsafe { SendInput(&inputs, size) }
}

fn pump_messages(wait: Duration) {
    let milliseconds = u32::try_from(wait.as_millis()).unwrap_or(u32::MAX);
    // SAFETY: waiting on no handles, only on this thread's message queue.
    unsafe { MsgWaitForMultipleObjects(None, false, milliseconds, QS_ALLINPUT) };
    let mut message = MSG::default();
    // SAFETY: `message` is writable, and every message is dispatched on the thread that owns
    // its window.
    unsafe {
        while PeekMessageW(&raw mut message, None, 0, 0, PM_REMOVE).as_bool() {
            // TranslateMessage returns whether it made a character message, not an error.
            let _ = TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
    }
}

/// Ctrl+Shift+7, the full-screen hotkey the field test's runs give cleanshot.exe (F-07).
const FULL_SCREEN: [VIRTUAL_KEY; 3] = [VK_CONTROL, VK_SHIFT, VIRTUAL_KEY(0x37)];

/// F-20 beside clipboard history: presses [`FULL_SCREEN`] `times` for the running
/// cleanshot.exe to copy the screen while another thread reads the clipboard on every change,
/// as Win+V clipboard history and clipboard managers do. Returns how many presses ended in a
/// copy that `copies` counted within `timeout`.
///
/// # Panics
///
/// If the watching thread panicked.
pub fn copies_while_watched(times: u32, timeout: Duration, mut copies: impl FnMut() -> u32) -> u32 {
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = Arc::clone(&stop);
    let watcher = std::thread::spawn(move || {
        let mut seen = clipboard_sequence();
        while !stopped.load(Ordering::Relaxed) {
            let now = clipboard_sequence();
            if now != seen {
                seen = now;
                // Like a clipboard manager, a read this change refuses is not retried; the next
                // change is read instead. Only cleanshot.exe's copies are counted.
                let _ = crate::clipboard::read(u32::from(CF_DIBV5.0));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let mut delivered = 0;
    for _ in 0..times {
        let before = copies();
        if press(&FULL_SCREEN) == 6 && within(timeout, || copies() > before) {
            delivered += 1;
        }
    }
    stop.store(true, Ordering::Relaxed);
    watcher.join().expect("the watching thread does not panic");
    delivered
}

fn clipboard_sequence() -> u32 {
    // SAFETY: GetClipboardSequenceNumber has no preconditions.
    unsafe { GetClipboardSequenceNumber() }
}

/// Puts `text` on the clipboard as Unicode text, then reads the clipboard's text back.
///
/// # Errors
///
/// The Windows error of the first clipboard call that fails.
pub fn clipboard_round_trip(text: &str) -> Result<String, windows::core::Error> {
    write_clipboard(text)?;
    read_clipboard()
}

fn write_clipboard(text: &str) -> Result<(), windows::core::Error> {
    let bytes: Vec<u8> = text
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    crate::clipboard::replace_with(&[(u32::from(CF_UNICODETEXT.0), &bytes)])
}

fn read_clipboard() -> Result<String, windows::core::Error> {
    let bytes = crate::clipboard::read(u32::from(CF_UNICODETEXT.0))?;
    let text: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&unit| u16::from_le_bytes(unit))
        .take_while(|&unit| unit != 0)
        .collect();
    Ok(String::from_utf16_lossy(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p04_a_capture_device_is_priced_on_every_adapter_that_shows_the_desktop() {
        let costs = capture_device_costs().expect("the devices are made");
        assert!(!costs.is_empty(), "the desktop is on some adapter");
        assert!(
            costs.iter().all(|cost| cost.made > Duration::ZERO),
            "{costs:?}"
        );
    }

    #[test]
    fn the_probe_names_windows_11_from_build_22000_whatever_the_registry_product_name_says() {
        let named = |build| {
            WindowsVersion {
                display_version: String::from("25H2"),
                build,
                revision: 0,
                edition: String::from("Enterprise"),
                installation_type: String::from("Client"),
            }
            .name()
        };
        assert_eq!(
            [19045, 21999, 22000, 26200].map(named),
            ["Windows 10", "Windows 10", "Windows 11", "Windows 11"]
        );
    }
}
