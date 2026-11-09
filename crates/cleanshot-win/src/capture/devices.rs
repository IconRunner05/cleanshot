use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HMODULE, LUID};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device,
    ID3D11DeviceContext,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_NOT_FOUND, IDXGIAdapter1, IDXGIDevice3, IDXGIFactory1,
};
use windows::core::Interface;

use crate::Error;

/// Where an adapter's Direct3D device came from for one copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Device {
    /// The adapter shows no screen, so it got none: a device wakes a sleeping GPU.
    Skipped,
    /// Making one failed; the copy's error says why.
    Failed,
    /// Made for this copy, taking this long.
    Made { took: Duration, because: Fresh },
    /// The device the last copy on this adapter made, kept by [`Devices`].
    Kept,
}

/// Why an adapter showing the desktop got a new device rather than a kept one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fresh {
    /// None was kept for this adapter.
    NoneKept,
    /// The kept one had been removed, as by a driver update or a GPU reset.
    Removed,
    /// The kept one could not duplicate this adapter's outputs.
    Retried,
}

/// The Direct3D device of each adapter that showed the desktop, kept from one copy to
/// the next so a copy skips making one. A kept device holds no pixels between copies,
/// and one whose copy failed is not kept, so the next copy makes it afresh.
#[derive(Default)]
pub struct Devices {
    pub(super) kept: Vec<(LUID, ID3D11Device, ID3D11DeviceContext)>,
}

impl Devices {
    /// Lets every kept device go, with what its driver holds, so the next copy makes its
    /// own. Each is trimmed first, which has its driver drop the buffers it keeps to speed up
    /// the next copy. Returns how many it let go.
    ///
    /// # Panics
    ///
    /// If a kept device is not a DXGI 1.3 device, which Windows 11 rules out.
    pub fn release(&mut self) -> usize {
        let count = self.kept.len();
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
        count
    }

    /// Makes and keeps a device for each adapter showing the desktop now, so the first
    /// copy after start reuses it.
    ///
    /// # Errors
    ///
    /// The Direct3D or DXGI error if an adapter cannot be listed or a device made; the devices
    /// made before it stay kept.
    pub fn prewarm(&mut self) -> Result<(), Error> {
        for (_, adapter) in desktop_adapters().map_err(Error::Capture)? {
            // SAFETY: a read of the adapter's own description.
            let luid = unsafe { adapter.GetDesc1() }
                .map_err(Error::Capture)?
                .AdapterLuid;
            let (device, context, _) = make_device(&adapter).map_err(Error::Capture)?;
            self.kept.push((luid, device, context));
        }
        Ok(())
    }

    pub(super) fn take(&mut self, adapter: LUID) -> Option<(ID3D11Device, ID3D11DeviceContext)> {
        let at = self.kept.iter().position(|(kept, _, _)| *kept == adapter)?;
        let (_, device, context) = self.kept.swap_remove(at);
        Some((device, context))
    }
}

/// `adapter`'s LUID when it shows the desktop, the adapters a kept device may be reused on.
pub(super) fn desktop_luid(adapter: &IDXGIAdapter1) -> Option<LUID> {
    // SAFETY: a read of the adapter's own description.
    let luid = unsafe { adapter.GetDesc1() }.ok()?.AdapterLuid;
    shows_desktop(adapter).ok()?.then_some(luid)
}

/// A kept device is reused only while Direct3D says it has not been removed.
pub(super) fn reusable(removed: windows::core::Result<()>) -> Result<(), Fresh> {
    removed.map_err(|_removed| Fresh::Removed)
}

pub(crate) fn make_device(
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

/// Each adapter with an output on the desktop, with its place in DXGI's list.
pub(crate) fn desktop_adapters() -> windows::core::Result<Vec<(u32, IDXGIAdapter1)>> {
    // SAFETY: CreateDXGIFactory1 has no preconditions.
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }?;
    let mut found = Vec::new();
    for index in 0.. {
        // SAFETY: an index past the last adapter returns DXGI_ERROR_NOT_FOUND.
        let adapter = match unsafe { factory.EnumAdapters1(index) } {
            Ok(adapter) => adapter,
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(error) => return Err(error),
        };
        if shows_desktop(&adapter)? {
            found.push((index, adapter));
        }
    }
    Ok(found)
}

fn shows_desktop(adapter: &IDXGIAdapter1) -> windows::core::Result<bool> {
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

#[cfg(test)]
mod tests {
    use windows::Win32::Graphics::Dxgi::DXGI_ERROR_DEVICE_REMOVED;

    use super::*;

    #[test]
    fn a_kept_device_is_reused_only_while_it_has_not_been_removed() {
        assert_eq!(reusable(Ok(())), Ok(()));
        let removed = windows::core::Error::from(DXGI_ERROR_DEVICE_REMOVED);
        assert_eq!(reusable(Err(removed)), Err(Fresh::Removed));
    }
}
