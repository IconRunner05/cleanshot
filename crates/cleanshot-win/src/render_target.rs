use std::cell::RefCell;

use cleanshot_core::RgbaImage;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_SIZE_U, D2D1_ALPHA_MODE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_PROPERTIES, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_HWND_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1CreateFactory, ID2D1Bitmap, ID2D1Factory,
    ID2D1HwndRenderTarget,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;

use crate::capture::DESKTOP_DPI;

const FORMAT: D2D1_PIXEL_FORMAT = D2D1_PIXEL_FORMAT {
    format: DXGI_FORMAT_B8G8R8A8_UNORM,
    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
};

thread_local! {
    static FACTORY: RefCell<Option<ID2D1Factory>> = const { RefCell::new(None) };
}

/// P-01: this thread's Direct2D factory, made on first use and kept until
/// [`release_factory`], so every window's render target shares the device Direct2D made for
/// the first one instead of making its own.
pub(crate) fn factory() -> windows::core::Result<ID2D1Factory> {
    FACTORY.with_borrow_mut(|kept| {
        if let Some(factory) = kept {
            return Ok(factory.clone());
        }
        // SAFETY: D2D1CreateFactory has no preconditions.
        let factory: ID2D1Factory =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }?;
        Ok(kept.insert(factory).clone())
    })
}

/// P-04: lets go of this thread's Direct2D factory, so the device it keeps is freed once no
/// window holds a render target from it; the next window makes both afresh.
pub fn release_factory() {
    FACTORY.set(None);
}

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
/// `height` of `bgra`, rows of BGRA `width` pixels long whose alpha is read as `alpha` says.
pub(crate) fn target_and_bitmap(
    factory: &ID2D1Factory,
    window: HWND,
    size: D2D_SIZE_U,
    (width, height): (u32, u32),
    bgra: &[u8],
    alpha: D2D1_ALPHA_MODE,
) -> windows::core::Result<(ID2D1HwndRenderTarget, ID2D1Bitmap)> {
    let bitmap_properties = D2D1_BITMAP_PROPERTIES {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: alpha,
        },
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

#[cfg(test)]
mod tests {
    use windows::core::Interface;

    use super::*;

    #[test]
    fn p01_every_window_on_a_thread_draws_through_one_direct2d_factory() {
        let first = factory().expect("Direct2D starts");
        let second = factory().expect("Direct2D starts");
        assert_eq!(first.as_raw(), second.as_raw());
    }

    #[test]
    fn p04_a_released_factory_is_let_go_and_the_next_window_makes_another() {
        let first = factory().expect("Direct2D starts");
        release_factory();
        assert!(
            FACTORY.with_borrow(Option::is_none),
            "the thread keeps no factory"
        );
        let second = factory().expect("Direct2D starts again");
        assert_ne!(first.as_raw(), second.as_raw());
    }
}
