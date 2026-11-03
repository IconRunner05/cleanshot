use cleanshot_core::RgbaImage;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_SIZE_U, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_PROPERTIES, D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_PROPERTIES,
    ID2D1Bitmap, ID2D1Factory, ID2D1HwndRenderTarget,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;

use crate::capture::DESKTOP_DPI;

const FORMAT: D2D1_PIXEL_FORMAT = D2D1_PIXEL_FORMAT {
    format: DXGI_FORMAT_B8G8R8A8_UNORM,
    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
};

/// A render target for `window`, `target` physical pixels.
pub(crate) fn target(
    factory: &ID2D1Factory,
    window: HWND,
    target: D2D_SIZE_U,
) -> windows::core::Result<ID2D1HwndRenderTarget> {
    let target_properties = D2D1_RENDER_TARGET_PROPERTIES {
        pixelFormat: FORMAT,
        dpiX: DESKTOP_DPI,
        dpiY: DESKTOP_DPI,
        ..Default::default()
    };
    let window_properties = D2D1_HWND_RENDER_TARGET_PROPERTIES {
        hwnd: window,
        pixelSize: target,
        ..Default::default()
    };
    // SAFETY: both descriptions are fully initialised.
    unsafe {
        factory.CreateHwndRenderTarget(&raw const target_properties, &raw const window_properties)
    }
}

/// A render target for `window`, `target` physical pixels, and a bitmap on it `width` x
/// `height` of `bgra`, rows of premultiplied BGRA `width` pixels long.
pub(crate) fn target_and_bitmap(
    factory: &ID2D1Factory,
    window: HWND,
    size: D2D_SIZE_U,
    (width, height): (u32, u32),
    bgra: &[u8],
) -> windows::core::Result<(ID2D1HwndRenderTarget, ID2D1Bitmap)> {
    let bitmap_properties = D2D1_BITMAP_PROPERTIES {
        pixelFormat: FORMAT,
        dpiX: DESKTOP_DPI,
        dpiY: DESKTOP_DPI,
    };
    assert_eq!(
        bgra.len(),
        width as usize * height as usize * 4,
        "bgra is width x height pixels"
    );
    let target = target(factory, window, size)?;
    // SAFETY: the description is fully initialised; `bgra` holds width * height * 4 bytes
    // with a pitch of width * 4, and Direct2D copies it during CreateBitmap.
    unsafe {
        let frozen = target.CreateBitmap(
            D2D_SIZE_U { width, height },
            Some(bgra.as_ptr().cast()),
            width * 4,
            &raw const bitmap_properties,
        )?;
        Ok((target, frozen))
    }
}

/// The pixels of an opaque `image` as premultiplied BGRA. Captures are opaque, so straight and
/// premultiplied alpha agree and only the channel order changes.
pub(crate) fn bgra(image: &RgbaImage) -> Vec<u8> {
    let mut bgra = image.pixels().to_vec();
    for pixel in bgra.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    bgra
}
