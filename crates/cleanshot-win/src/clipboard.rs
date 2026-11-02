use std::time::Duration;

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
#[cfg(any(test, feature = "field-test"))]
use windows::Win32::System::DataExchange::GetClipboardData;
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
#[cfg(any(test, feature = "field-test"))]
use windows::Win32::System::Memory::GlobalSize;
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_DIBV5;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE,
};
use windows::core::{PCWSTR, w};

/// How many times a copy tries to open the clipboard while another program holds it, and the
/// wait between tries: 10 tries over 90 ms, inside P-02's 150 ms.
const OPEN_ATTEMPTS: u32 = 10;
const OPEN_RETRY: Duration = Duration::from_millis(10);

/// Replaces the clipboard's contents with one image in two formats (F-20): `bitmap`, a
/// packed `CF_DIBV5`, and `png`, a PNG file under the registered "PNG" format.
///
/// # Errors
///
/// The Windows error if another program holds the clipboard or Windows refuses the data.
pub fn copy_image(bitmap: &[u8], png: &[u8]) -> windows::core::Result<()> {
    replace_with(&[(u32::from(CF_DIBV5.0), bitmap), (png_format()?, png)])
}

pub(crate) fn png_format() -> windows::core::Result<u32> {
    registered(w!("PNG"))
}

/// The clipboard format Windows registers under `name`, the same in every program.
pub(crate) fn registered(name: PCWSTR) -> windows::core::Result<u32> {
    // SAFETY: the caller's name is a 'static NUL-terminated string.
    match unsafe { RegisterClipboardFormatW(name) } {
        0 => Err(windows::core::Error::from_thread()),
        format => Ok(format),
    }
}

/// Empties the clipboard and puts each `(format, bytes)` on it. A format that fails leaves
/// the ones before it on the clipboard.
pub(crate) fn replace_with(formats: &[(u32, &[u8])]) -> windows::core::Result<()> {
    while_open(|| {
        // SAFETY: the clipboard is open on this thread for the whole call.
        unsafe { EmptyClipboard() }?;
        formats
            .iter()
            .try_for_each(|&(format, bytes)| set(format, bytes))
    })
}

/// Runs `work` with the clipboard open on a message-only window of this thread's, then
/// closes it. A clipboard opened on no window has no owner after `EmptyClipboard`, so another
/// program can open it in the middle of the write and `SetClipboardData` fails; opened on a
/// window, the other program is refused until it is closed. A clipboard another program
/// holds is tried [`OPEN_ATTEMPTS`] times.
fn while_open<T>(work: impl FnOnce() -> windows::core::Result<T>) -> windows::core::Result<T> {
    // SAFETY: STATIC is a system class, and a message-only window needs no instance.
    let window = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("STATIC"),
            None,
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            None,
            None,
        )
    }?;
    let done = open_on(window).and_then(|()| {
        let worked = work();
        // SAFETY: the clipboard was opened on this thread by open_on.
        let closed = unsafe { CloseClipboard() };
        worked.and_then(|value| closed.map(|()| value))
    });
    // SAFETY: the window was created above on this thread and is destroyed once, here.
    let destroyed = unsafe { DestroyWindow(window) };
    done.and_then(|value| destroyed.map(|()| value))
}

fn open_on(window: HWND) -> windows::core::Result<()> {
    let mut attempt = 1;
    loop {
        // SAFETY: `window` is a live window of this thread.
        match unsafe { OpenClipboard(Some(window)) } {
            Ok(()) => return Ok(()),
            Err(_) if attempt < OPEN_ATTEMPTS => {
                attempt += 1;
                std::thread::sleep(OPEN_RETRY);
            }
            Err(error) => return Err(error),
        }
    }
}

/// Puts `bytes` on the open clipboard as `format`. The clipboard owns the block only once
/// `SetClipboardData` takes it, so a failure frees it here.
fn set(format: u32, bytes: &[u8]) -> windows::core::Result<()> {
    let block = global(bytes)?;
    // SAFETY: `block` is ours until SetClipboardData takes it, and freed here if it does not.
    unsafe {
        SetClipboardData(format, Some(HANDLE(block.0)))
            .map(|_| ())
            .map_err(|error| freed(block, error))
    }
}

/// A movable global block holding a copy of `bytes`, owned by the caller.
pub(crate) fn global(bytes: &[u8]) -> windows::core::Result<HGLOBAL> {
    // SAFETY: the block is allocated for `bytes.len()` bytes and written only through a
    // non-null lock of it; it is freed if it cannot be locked.
    unsafe {
        let block: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE, bytes.len())?;
        let target = GlobalLock(block).cast::<u8>();
        if target.is_null() {
            return Err(freed(block, windows::core::Error::from_thread()));
        }
        target.copy_from_nonoverlapping(bytes.as_ptr(), bytes.len());
        // GlobalUnlock reports an error when the lock count reaches zero, which is
        // the expected outcome here.
        let _ = GlobalUnlock(block);
        Ok(block)
    }
}

/// The block the clipboard holds as `format`, whole: Windows may round it up past the bytes
/// that were put there.
#[cfg(any(test, feature = "field-test"))]
pub(crate) fn read(format: u32) -> windows::core::Result<Vec<u8>> {
    while_open(|| {
        // SAFETY: the clipboard is open on this thread; the block is read only while locked
        // and only within its own size.
        unsafe {
            let block = HGLOBAL(GetClipboardData(format)?.0);
            let start = GlobalLock(block).cast::<u8>();
            if start.is_null() {
                return Err(windows::core::Error::from_thread());
            }
            let bytes = std::slice::from_raw_parts(start, GlobalSize(block)).to_vec();
            // GlobalUnlock reports an error when the lock count reaches zero, which is
            // the expected outcome here.
            let _ = GlobalUnlock(block);
            Ok(bytes)
        }
    })
}

/// Frees `block` and gives back `error`, the failure that kept the clipboard from taking it.
unsafe fn freed(block: HGLOBAL, error: windows::core::Error) -> windows::core::Error {
    // SAFETY: the caller owns `block`, unlocked, and no clipboard holds it. A block that will
    // not free is lost whatever is done, so `error` is the one the caller can act on.
    let _ = unsafe { GlobalFree(Some(block)) };
    error
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f20_copy_puts_the_bitmap_and_the_png_on_the_clipboard_unchanged() {
        let bitmap: Vec<u8> = (0..=255).collect();
        let png: Vec<u8> = (0..=255).rev().collect();
        copy_image(&bitmap, &png).expect("the clipboard takes the image");
        let bitmap_read = read(u32::from(CF_DIBV5.0)).expect("the bitmap is on the clipboard");
        let png_read = read(png_format().expect("PNG registers")).expect("the PNG is too");
        assert_eq!(bitmap_read[..bitmap.len()], bitmap);
        assert_eq!(png_read[..png.len()], png);
    }
}
