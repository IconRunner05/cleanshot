use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HGLOBAL, HMODULE, RECT};
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
use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    VIRTUAL_KEY, VK_CONTROL, VK_ESCAPE, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, FindWindowW, GetForegroundWindow, MSG, MsgWaitForMultipleObjects, PM_REMOVE,
    PeekMessageW, QS_ALLINPUT, TranslateMessage,
};
use windows::core::{Interface, PCWSTR, w};

pub use crate::fade::animations_on;
use crate::overlay::WINDOW_CLASS;

use crate::Error;
use crate::capture::monitor_handles;
use cleanshot_core::Hotkey;

use crate::tray::{Request, Tray};

/// The Windows release, read from the registry.
#[derive(Debug)]
pub struct WindowsVersion {
    pub product_name: String,
    pub display_version: String,
    pub build: String,
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
        product_name: registry_string(w!("ProductName"))?,
        display_version: registry_string(w!("DisplayVersion"))?,
        build: registry_string(w!("CurrentBuildNumber"))?,
        revision: registry_number(w!("UBR"))?,
        edition: registry_string(w!("EditionID"))?,
        installation_type: registry_string(w!("InstallationType"))?,
    })
}

const CURRENT_VERSION: PCWSTR = w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion");

fn registry_string(value: PCWSTR) -> Result<String, windows::core::Error> {
    let mut buffer = [0u16; 256];
    let mut bytes = u32::try_from(size_of_val(&buffer)).expect("512 fits in u32");
    // SAFETY: `buffer` is writable for `bytes` bytes, and RRF_RT_REG_SZ NUL-terminates it.
    unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            CURRENT_VERSION,
            value,
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&raw mut bytes),
        )
    }
    .ok()?;
    let length = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    Ok(String::from_utf16_lossy(&buffer[..length]))
}

fn registry_number(value: PCWSTR) -> Result<u32, windows::core::Error> {
    let mut number = 0u32;
    let mut bytes = u32::try_from(size_of::<u32>()).expect("4 fits in u32");
    // SAFETY: `number` is writable for the 4 bytes a REG_DWORD holds.
    unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            CURRENT_VERSION,
            value,
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut number).cast()),
            Some(&raw mut bytes),
        )
    }
    .ok()?;
    Ok(number)
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
    info.monitorInfo.cbSize = u32::try_from(size_of::<MONITORINFOEXW>()).expect("fits in u32");
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
    let tray = Tray::create(&[(Request::CaptureRegion, region)], move |request| {
        if request == Request::CaptureRegion {
            seen.set(true);
        }
    })?;
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
            let _ = TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
    }
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
    // SAFETY: the clipboard is opened and closed here; the data handle is read only while
    // locked, and only up to its NUL.
    unsafe {
        OpenClipboard(None)?;
        let read = GetClipboardData(u32::from(CF_UNICODETEXT.0)).and_then(|handle| {
            let block = HGLOBAL(handle.0);
            let start = GlobalLock(block).cast::<u16>();
            if start.is_null() {
                return Err(windows::core::Error::from_thread());
            }
            let mut length = 0;
            while *start.add(length) != 0 {
                length += 1;
            }
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(start, length));
            let _ = GlobalUnlock(block);
            Ok(text)
        });
        CloseClipboard()?;
        read
    }
}
