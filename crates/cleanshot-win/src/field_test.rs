use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HANDLE, HGLOBAL, HMODULE, RECT};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_ERROR_NOT_FOUND, IDXGIFactory1,
};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, HMONITOR, MONITORINFO, MONITORINFOEXW};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI,
    SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    VIRTUAL_KEY, VK_CONTROL, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetForegroundWindow, MSG, MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW,
    QS_ALLINPUT, TranslateMessage,
};
use windows::core::{PCWSTR, w};

use crate::Error;
use crate::capture::monitor_handles;
use crate::tray::{Hotkey, Request, Tray};

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
    pub hotkeys: Vec<(Hotkey, Result<(), String>)>,
    pub foreground_window: bool,
    pub inputs_sent: u32,
    pub hotkey_delivered_after: Option<Duration>,
}

/// Makes this process per-monitor DPI aware (V2), so [`monitors`] reports real DPI.
///
/// # Errors
///
/// The Windows error if the awareness was already set or is refused.
pub fn make_dpi_aware() -> Result<(), windows::core::Error> {
    // SAFETY: a constant awareness context, set before any window exists.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
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
#[must_use]
pub fn monitors() -> Vec<Monitor> {
    monitor_handles()
        .into_iter()
        .map(describe_monitor)
        .collect()
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
    Monitor {
        device: if described.as_bool() {
            String::from_utf16_lossy(&info.szDevice[..length])
        } else {
            String::from("(GetMonitorInfoW failed)")
        },
        bounds: info.monitorInfo.rcMonitor,
        dpi,
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
/// # Errors
///
/// [`Error::Window`] if the tray window cannot be created.
pub fn drive_tray(timeout: Duration) -> Result<TrayRun, Error> {
    let delivered = Rc::new(Cell::new(false));
    let seen = Rc::clone(&delivered);
    let tray = Tray::create(move |request| {
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
        hotkeys: tray
            .hotkeys()
            .iter()
            .map(|(hotkey, result)| {
                (
                    *hotkey,
                    result.as_ref().copied().map_err(ToString::to_string),
                )
            })
            .collect(),
        foreground_window,
        inputs_sent,
        hotkey_delivered_after: delivered.get().then(|| start.elapsed()),
    })
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
    let units: Vec<u16> = text.encode_utf16().chain([0]).collect();
    // SAFETY: the clipboard is opened and closed here on one thread; the global block is
    // sized for `units`, filled while locked, and owned by the clipboard once set.
    unsafe {
        OpenClipboard(None)?;
        let written = EmptyClipboard().and_then(|()| {
            let block: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE, size_of_val(units.as_slice()))?;
            let target = GlobalLock(block).cast::<u16>();
            target.copy_from_nonoverlapping(units.as_ptr(), units.len());
            let _ = GlobalUnlock(block);
            SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(block.0))).map(|_| ())
        });
        CloseClipboard()?;
        written
    }
}

fn read_clipboard() -> Result<String, windows::core::Error> {
    // SAFETY: the clipboard is opened and closed here; the data handle is read only while
    // locked, and only up to its NUL.
    unsafe {
        OpenClipboard(None)?;
        let read = GetClipboardData(u32::from(CF_UNICODETEXT.0)).map(|handle| {
            let block = HGLOBAL(handle.0);
            let start = GlobalLock(block).cast::<u16>();
            let mut length = 0;
            while *start.add(length) != 0 {
                length += 1;
            }
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(start, length));
            let _ = GlobalUnlock(block);
            text
        });
        CloseClipboard()?;
        read
    }
}
