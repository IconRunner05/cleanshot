use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
#[cfg(any(test, feature = "field-test"))]
use windows::Win32::System::DataExchange::GetClipboardData;
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
#[cfg(any(test, feature = "field-test"))]
use windows::Win32::System::Memory::GlobalSize;
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_DIBV5;
use windows::core::w;

/// Replaces the clipboard's contents with one image in two formats (F-20): `bitmap`, a
/// packed `CF_DIBV5`, and `png`, a PNG file under the registered "PNG" format.
///
/// # Errors
///
/// The Windows error if another program holds the clipboard or Windows refuses the data.
pub fn copy_image(bitmap: &[u8], png: &[u8]) -> windows::core::Result<()> {
    replace_with(&[(u32::from(CF_DIBV5.0), bitmap), (png_format()?, png)])
}

fn png_format() -> windows::core::Result<u32> {
    // SAFETY: the name is a 'static NUL-terminated string.
    match unsafe { RegisterClipboardFormatW(w!("PNG")) } {
        0 => Err(windows::core::Error::from_thread()),
        format => Ok(format),
    }
}

/// Empties the clipboard and puts each `(format, bytes)` on it. A format that fails leaves
/// the ones before it on the clipboard.
pub(crate) fn replace_with(formats: &[(u32, &[u8])]) -> windows::core::Result<()> {
    // SAFETY: the clipboard is opened and closed here on one thread.
    unsafe {
        OpenClipboard(None)?;
        let written = EmptyClipboard().and_then(|()| {
            formats
                .iter()
                .try_for_each(|&(format, bytes)| set(format, bytes))
        });
        CloseClipboard()?;
        written
    }
}

/// Puts `bytes` on the open clipboard as `format`. The clipboard owns the block only once
/// `SetClipboardData` takes it, so every failure before that frees it here.
fn set(format: u32, bytes: &[u8]) -> windows::core::Result<()> {
    // SAFETY: the block is allocated for `bytes.len()` bytes and written only through a
    // non-null lock of it; it is freed on every path where the clipboard has not taken it.
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
        SetClipboardData(format, Some(HANDLE(block.0)))
            .map(|_| ())
            .map_err(|error| freed(block, error))
    }
}

/// The block the clipboard holds as `format`, whole: Windows may round it up past the bytes
/// that were put there.
#[cfg(any(test, feature = "field-test"))]
pub(crate) fn read(format: u32) -> windows::core::Result<Vec<u8>> {
    // SAFETY: the clipboard is opened and closed here; the block is read only while locked
    // and only within its own size.
    unsafe {
        OpenClipboard(None)?;
        let read = GetClipboardData(format).and_then(|handle| {
            let block = HGLOBAL(handle.0);
            let start = GlobalLock(block).cast::<u8>();
            if start.is_null() {
                return Err(windows::core::Error::from_thread());
            }
            let bytes = std::slice::from_raw_parts(start, GlobalSize(block)).to_vec();
            // GlobalUnlock reports an error when the lock count reaches zero, which is
            // the expected outcome here.
            let _ = GlobalUnlock(block);
            Ok(bytes)
        });
        CloseClipboard()?;
        read
    }
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
